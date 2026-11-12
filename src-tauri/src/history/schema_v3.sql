ALTER TABLE performance_events ADD COLUMN interrupted INTEGER NOT NULL DEFAULT 0
    CHECK (interrupted IN (0, 1) AND (interrupted = 0 OR end_sequence IS NULL));
