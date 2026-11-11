CREATE TABLE recording_sessions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    started_at_ms INTEGER NOT NULL CHECK (started_at_ms >= 0)
) STRICT;

CREATE TABLE metric_samples (
    session_id INTEGER NOT NULL REFERENCES recording_sessions(id) ON DELETE CASCADE,
    sequence INTEGER NOT NULL CHECK (sequence > 0),
    timestamp_ms INTEGER NOT NULL CHECK (timestamp_ms >= 0),
    monotonic_ms INTEGER NOT NULL CHECK (monotonic_ms >= 0),
    interval_ms INTEGER NOT NULL CHECK (interval_ms IN (1000, 2000, 5000, 10000)),
    cpu_percent REAL CHECK (cpu_percent >= 0 AND cpu_percent <= 100),
    memory_used_bytes INTEGER,
    memory_total_bytes INTEGER,
    battery_percent REAL CHECK (battery_percent >= 0 AND battery_percent <= 100),
    battery_state TEXT,
    sample_status TEXT NOT NULL CHECK (sample_status IN ('healthy', 'warmingUp', 'degraded', 'error')),
    battery_status TEXT NOT NULL CHECK (battery_status IN ('available', 'absent', 'error', 'unavailable')),
    PRIMARY KEY (session_id, sequence),
    CHECK (
        (memory_used_bytes IS NULL AND memory_total_bytes IS NULL)
        OR (memory_used_bytes IS NOT NULL AND memory_total_bytes IS NOT NULL
            AND memory_used_bytes >= 0 AND memory_total_bytes > 0
            AND memory_used_bytes <= memory_total_bytes)
    ),
    CHECK ((battery_percent IS NULL) = (battery_state IS NULL)),
    CHECK (battery_status = 'available' OR battery_percent IS NULL),
    CHECK (sample_status != 'error' OR
        (cpu_percent IS NULL AND memory_used_bytes IS NULL AND battery_percent IS NULL))
) STRICT;

CREATE INDEX metric_samples_timestamp ON metric_samples(timestamp_ms, session_id, sequence);
