use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, Read, Seek, SeekFrom, Write};
use std::ops::Range;

const MAGIC: &[u8; 8] = b"UWURPT01";
const BLOCK_BYTES: usize = 256 * 1024;
const MAX_EVENT_BYTES: usize = 1024 * 1024;
const MAX_INDEX_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Deserialize, Serialize)]
struct Block {
    offset: u64,
    stored_bytes: u32,
    raw_bytes: u32,
    first_event: u64,
    events: u32,
}

#[derive(Deserialize, Serialize)]
struct Index {
    blocks: Vec<Block>,
    events: u64,
}

pub fn write_report(mut events: impl BufRead, mut output: impl Write) -> io::Result<u64> {
    output.write_all(MAGIC)?;
    let mut buffer = Vec::with_capacity(BLOCK_BYTES);
    let mut event = Vec::new();
    let mut index = Index {
        blocks: Vec::new(),
        events: 0,
    };
    let mut offset = MAGIC.len() as u64;
    let mut block_events = 0;
    loop {
        event.clear();
        let read = (&mut events)
            .take(MAX_EVENT_BYTES as u64 + 1)
            .read_until(b'\n', &mut event)?;
        if read > MAX_EVENT_BYTES {
            return Err(invalid("combat log event exceeds the size limit"));
        }
        if !buffer.is_empty() && (read == 0 || buffer.len() + read > BLOCK_BYTES) {
            let compressed = zstd::bulk::compress(&buffer, 3)?;
            output.write_all(&compressed)?;
            index.blocks.push(Block {
                offset,
                stored_bytes: compressed.len() as u32,
                raw_bytes: buffer.len() as u32,
                first_event: index.events,
                events: block_events,
            });
            index.events += u64::from(block_events);
            offset += compressed.len() as u64;
            buffer.clear();
            block_events = 0;
        }
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&event);
        block_events += 1;
    }
    if index.events == 0 {
        return Err(invalid("empty combat log"));
    }
    let footer = serde_json::to_vec(&index).map_err(io::Error::other)?;
    if footer.len() as u64 > MAX_INDEX_BYTES {
        return Err(invalid("combat log index exceeds the size limit"));
    }
    output.write_all(&footer)?;
    output.write_all(&(footer.len() as u64).to_le_bytes())?;
    Ok(index.events)
}

pub struct ReportFile<R> {
    input: R,
    index: Index,
}

impl<R: Read + Seek> ReportFile<R> {
    pub fn open(mut input: R) -> io::Result<Self> {
        let mut bytes = [0; 8];
        input.read_exact(&mut bytes)?;
        if &bytes != MAGIC {
            return Err(invalid("unknown report file version"));
        }
        let footer_end = input.seek(SeekFrom::End(-8))?;
        input.read_exact(&mut bytes)?;
        let length = u64::from_le_bytes(bytes);
        let footer_start = footer_end
            .checked_sub(length)
            .filter(|position| *position >= MAGIC.len() as u64 && length <= MAX_INDEX_BYTES)
            .ok_or_else(|| invalid("invalid report index size"))?;
        input.seek(SeekFrom::Start(footer_start))?;
        let mut footer = vec![0; length as usize];
        input.read_exact(&mut footer)?;
        let index: Index = serde_json::from_slice(&footer).map_err(io::Error::other)?;
        let mut offset = MAGIC.len() as u64;
        let mut events = 0;
        for block in &index.blocks {
            if block.offset != offset
                || block.first_event != events
                || block.events == 0
                || block.raw_bytes == 0
                || block.raw_bytes as usize > MAX_EVENT_BYTES
                || block.stored_bytes == 0
                || block.stored_bytes as usize > MAX_EVENT_BYTES * 2
            {
                return Err(invalid("invalid report block index"));
            }
            offset += u64::from(block.stored_bytes);
            events += u64::from(block.events);
        }
        if offset != footer_start || events != index.events || events == 0 {
            return Err(invalid("report index does not match report data"));
        }
        Ok(Self { input, index })
    }

    pub fn event_count(&self) -> u64 {
        self.index.events
    }

    pub fn read_events(&mut self, range: Range<u64>, mut output: impl Write) -> io::Result<()> {
        if range.start > range.end || range.end > self.index.events {
            return Err(invalid("report event range is out of bounds"));
        }
        if range.is_empty() {
            return Ok(());
        }
        let first = self
            .index
            .blocks
            .partition_point(|block| block.first_event + u64::from(block.events) <= range.start);
        for block in &self.index.blocks[first..] {
            if block.first_event >= range.end {
                break;
            }
            self.input.seek(SeekFrom::Start(block.offset))?;
            let mut compressed = vec![0; block.stored_bytes as usize];
            self.input.read_exact(&mut compressed)?;
            let raw = zstd::bulk::decompress(&compressed, block.raw_bytes as usize)?;
            let block_end = block.first_event + u64::from(block.events);
            if raw.len() != block.raw_bytes as usize
                || raw.split_inclusive(|byte| *byte == b'\n').count() != block.events as usize
            {
                return Err(invalid("report block does not match its index"));
            }
            if range.start <= block.first_event && block_end <= range.end {
                output.write_all(&raw)?;
            } else {
                let start = range.start.saturating_sub(block.first_event) as usize;
                let end = (range.end.min(block_end) - block.first_event) as usize;
                for event in raw
                    .split_inclusive(|byte| *byte == b'\n')
                    .take(end)
                    .skip(start)
                {
                    output.write_all(event)?;
                }
            }
        }
        Ok(())
    }
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn one_report_preserves_all_events_and_reads_across_block_boundaries() {
        let large_event = format!("{}\n", "x".repeat(BLOCK_BYTES - 1));
        let last_event = "last event without a trailing newline";
        let log = format!("{large_event}after boundary\n{large_event}{last_event}");
        let mut data = Vec::new();
        assert_eq!(write_report(Cursor::new(&log), &mut data).unwrap(), 4);

        let mut report = ReportFile::open(Cursor::new(&data)).unwrap();
        let mut decoded = Vec::new();
        report
            .read_events(0..report.event_count(), &mut decoded)
            .unwrap();
        assert_eq!(decoded, log.as_bytes());
        decoded.clear();
        report.read_events(1..3, &mut decoded).unwrap();
        assert_eq!(decoded, format!("after boundary\n{large_event}").as_bytes());
        assert!(report.read_events(4..5, &mut decoded).is_err());
        assert!(ReportFile::open(Cursor::new(&data[..data.len() - 1])).is_err());
    }
}
