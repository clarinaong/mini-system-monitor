CREATE TABLE performance_events (
    session_id INTEGER NOT NULL REFERENCES recording_sessions(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('cpuSpike', 'sustainedCpu', 'sustainedMemory', 'memoryJump')),
    start_sequence INTEGER NOT NULL CHECK (start_sequence > 0),
    start_ms INTEGER NOT NULL CHECK (start_ms >= 0),
    start_monotonic_ms INTEGER NOT NULL CHECK (start_monotonic_ms >= 0),
    detected_sequence INTEGER NOT NULL CHECK (detected_sequence >= start_sequence),
    detected_ms INTEGER NOT NULL CHECK (detected_ms >= 0),
    detected_monotonic_ms INTEGER NOT NULL CHECK (detected_monotonic_ms >= start_monotonic_ms),
    end_sequence INTEGER CHECK (end_sequence > detected_sequence),
    end_ms INTEGER CHECK (end_ms >= 0),
    end_monotonic_ms INTEGER CHECK (end_monotonic_ms > detected_monotonic_ms),
    CHECK ((end_sequence IS NULL) = (end_ms IS NULL) AND
           (end_sequence IS NULL) = (end_monotonic_ms IS NULL)),
    PRIMARY KEY (session_id, kind, start_sequence)
) STRICT;

CREATE INDEX performance_events_start ON performance_events(start_ms);
