import logs_core
from h_debug import running_time

class Timestamps(logs_core.Logs):
    @property
    def TIMESTAMPS(self):
        try:
            return self.__TIMESTAMPS
        except AttributeError:
            self.__TIMESTAMPS = self._get_timestamps()
            return self.__TIMESTAMPS
    
    def _get_timestamps(self):
        try:
            return self._read_timestamps()
        except Exception:
            return self._redo_timestamps()
    
    # @running_time
    def _read_timestamps(self):
        timestamp_data_file_name = self.relative_path("TIMESTAMP_DATA.json")
        return timestamp_data_file_name.json()
    
    @running_time
    def _redo_timestamps(self):
        timestamps = self._new_timestamps()
        timestamp_data_file_name = self.relative_path("TIMESTAMP_DATA.json")
        timestamp_data_file_name.json_write(timestamps)
        return timestamps
    
    def _new_timestamps(self):
        # A lockout may span several nights. Minute-only subtraction loses
        # complete hours/days and makes links select the wrong log rows.
        times: list[int] = []
        first_line = self.LOGS[0]
        times.append(0)
        previous = first_line.split('.', 1)[0]
        for n, line in enumerate(self.LOGS):
            current = line.split('.', 1)[0]
            if current == previous:
                continue
            try:
                elapsed = int(self.get_timedelta_seconds(first_line, line))
            except (ValueError, IndexError):
                continue
            if elapsed >= len(times):
                times.extend([n]*(elapsed-len(times)+1))
            previous = current
        return times
