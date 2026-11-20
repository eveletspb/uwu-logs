use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, Read, Seek, SeekFrom, Write};
use std::ops::Range;

const MAGIC: &[u8; 8] = b"UWURPT01";
const BLOCK_BYTES: usize = 1024 * 1024;
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
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    metadata: serde_json::Value,
}

impl Index {
    fn write_footer(&self, mut output: impl Write) -> io::Result<()> {
        let footer = serde_json::to_vec(self).map_err(io::Error::other)?;
        if footer.len() as u64 > MAX_INDEX_BYTES {
            return Err(invalid("combat log index exceeds the size limit"));
        }
        output.write_all(&footer)?;
        output.write_all(&(footer.len() as u64).to_le_bytes())
    }
}

pub(crate) fn read_event(input: &mut impl BufRead, event: &mut Vec<u8>) -> io::Result<usize> {
    event.clear();
    let read = input
        .take(MAX_EVENT_BYTES as u64 + 1)
        .read_until(b'\n', event)?;
    if read > MAX_EVENT_BYTES {
        return Err(invalid("combat log event exceeds the size limit"));
    }
    Ok(read)
}

pub struct ReportWriter<W> {
    output: W,
    index: Index,
    offset: u64,
    buffer: Vec<u8>,
    events_in_block: u32,
    compressor: zstd::bulk::Compressor<'static>,
}

impl<W: Write> ReportWriter<W> {
    pub fn new(mut output: W) -> io::Result<Self> {
        output.write_all(MAGIC)?;
        let mut compressor = zstd::bulk::Compressor::new(3)?;
        compressor.set_parameter(zstd::zstd_safe::CParameter::ChecksumFlag(true))?;
        Ok(Self {
            output,
            index: Index {
                blocks: Vec::new(),
                events: 0,
                metadata: serde_json::Value::Null,
            },
            offset: MAGIC.len() as u64,
            buffer: Vec::with_capacity(BLOCK_BYTES),
            events_in_block: 0,
            compressor,
        })
    }

    pub fn push_event(&mut self, event: &[u8]) -> io::Result<()> {
        if event.is_empty() || event.len() > MAX_EVENT_BYTES {
            return Err(invalid("invalid combat event size"));
        }
        if self.buffer.len() + event.len() > BLOCK_BYTES {
            self.flush_block()?;
        }
        self.buffer.extend_from_slice(event);
        self.events_in_block += 1;
        Ok(())
    }

    fn flush_block(&mut self) -> io::Result<()> {
        if self.events_in_block == 0 {
            return Ok(());
        }
        let compressed = self.compressor.compress(&self.buffer)?;
        self.output.write_all(&compressed)?;
        self.index.blocks.push(Block {
            offset: self.offset,
            stored_bytes: compressed.len() as u32,
            raw_bytes: self.buffer.len() as u32,
            first_event: self.index.events,
            events: self.events_in_block,
        });
        self.index.events += u64::from(self.events_in_block);
        self.offset += compressed.len() as u64;
        self.buffer.clear();
        self.events_in_block = 0;
        Ok(())
    }

    pub fn finish(mut self, metadata: serde_json::Value) -> io::Result<u64> {
        self.flush_block()?;
        if self.index.events == 0 {
            return Err(invalid("empty combat log"));
        }
        self.index.metadata = metadata;
        self.index.write_footer(&mut self.output)?;
        Ok(self.index.events)
    }
}

pub fn write_report(mut events: impl BufRead, output: impl Write) -> io::Result<u64> {
    let mut writer = ReportWriter::new(output)?;
    let mut event = Vec::new();
    while read_event(&mut events, &mut event)? != 0 {
        writer.push_event(&event)?;
    }
    writer.finish(serde_json::Value::Null)
}

pub struct LogStream<R> {
    report: ReportFile<R>,
    ranges: std::collections::VecDeque<Range<u64>>,
    buffer: io::Cursor<Vec<u8>>,
}

impl<R: Read + Seek> Read for LogStream<R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        if self.buffer.position() as usize == self.buffer.get_ref().len() {
            let Some(range) = self.ranges.front_mut() else {
                return Ok(0);
            };
            let next = self.report.index.blocks.partition_point(|block| {
                block.first_event + u64::from(block.events) <= range.start
            });
            let block = &self.report.index.blocks[next];
            let end = range.end.min(block.first_event + u64::from(block.events));
            let selected = range.start..end;
            range.start = end;
            if range.is_empty() {
                self.ranges.pop_front();
            }
            self.buffer.get_mut().clear();
            self.buffer.set_position(0);
            self.report.read_events(selected, self.buffer.get_mut())?;
        }
        self.buffer.read(output)
    }
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

    pub fn into_reader(self) -> LogStream<R> {
        let end = self.event_count();
        self.into_range_reader(std::iter::once(0..end))
            .expect("full report range")
    }

    pub fn into_range_reader(
        self,
        ranges: impl IntoIterator<Item = Range<u64>>,
    ) -> io::Result<LogStream<R>> {
        let ranges: std::collections::VecDeque<_> = ranges
            .into_iter()
            .filter(|range| !range.is_empty())
            .collect();
        if ranges.iter().any(|range| range.end > self.event_count()) {
            return Err(invalid("report event range is out of bounds"));
        }
        Ok(LogStream {
            report: self,
            ranges,
            buffer: io::Cursor::new(Vec::new()),
        })
    }

    pub fn event_count(&self) -> u64 {
        self.index.events
    }

    pub fn metadata(&self) -> &serde_json::Value {
        &self.index.metadata
    }

    pub(crate) fn copy_with_metadata(
        mut self,
        mut output: impl Write,
        metadata: serde_json::Value,
    ) -> io::Result<()> {
        let block = self.index.blocks.last().expect("validated nonempty report");
        let end = block.offset + u64::from(block.stored_bytes);
        self.input.seek(SeekFrom::Start(0))?;
        if io::copy(&mut self.input.take(end), &mut output)? != end {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        self.index.metadata = metadata;
        self.index.write_footer(output)
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
        let mut decompressor = zstd::bulk::Decompressor::new()?;
        let mut compressed = Vec::new();
        for block in &self.index.blocks[first..] {
            if block.first_event >= range.end {
                break;
            }
            self.input.seek(SeekFrom::Start(block.offset))?;
            compressed.resize(block.stored_bytes as usize, 0);
            self.input.read_exact(&mut compressed)?;
            let raw = decompressor.decompress(&compressed, block.raw_bytes as usize)?;
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
