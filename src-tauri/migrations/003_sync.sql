-- Sync metadata lives inside the encrypted Gmail database. Re-running is safe.
CREATE TABLE IF NOT EXISTS sync_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    phase TEXT NOT NULL DEFAULT 'idle' CHECK (phase IN ('idle','listing','bodies','attachments','history','reconciling','complete')),
    paused INTEGER NOT NULL DEFAULT 1 CHECK (paused IN (0,1)),
    generation INTEGER NOT NULL DEFAULT 0,
    identity_checked INTEGER NOT NULL DEFAULT 0 CHECK (identity_checked IN (0,1)),
    labels_checked INTEGER NOT NULL DEFAULT 0 CHECK (labels_checked IN (0,1)),
    baseline TEXT,
    cursor TEXT,
    list_page TEXT,
    listing_done INTEGER NOT NULL DEFAULT 0 CHECK (listing_done IN (0,1)),
    history_page TEXT,
    history_target TEXT,
    history_done INTEGER NOT NULL DEFAULT 0 CHECK (history_done IN (0,1)),
    attempts INTEGER NOT NULL DEFAULT 0,
    next_retry_at INTEGER,
    last_error TEXT
);
INSERT OR IGNORE INTO sync_state(id) VALUES(1);
CREATE TABLE IF NOT EXISTS provider_labels (
    label_id TEXT PRIMARY KEY REFERENCES labels(id) ON DELETE CASCADE,
    json BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS sync_seen (
    message_id TEXT PRIMARY KEY,
    generation INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS gmail_parts_pending ON gmail_parts(is_body,message_id,part_id) WHERE content IS NULL;
CREATE TABLE IF NOT EXISTS sync_work (
    message_id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('fetch','verify'))
);
