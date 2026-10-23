CREATE TABLE database_kind (kind TEXT PRIMARY KEY CHECK (kind = 'synthetic-fixtures'));
INSERT INTO database_kind VALUES ('synthetic-fixtures');
CREATE TABLE accounts (
    id TEXT PRIMARY KEY,
    provider TEXT NOT NULL CHECK (provider = 'fixture'),
    address TEXT NOT NULL
);
CREATE TABLE threads (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    provider_id TEXT NOT NULL,
    UNIQUE(account_id, provider_id)
);
CREATE TABLE messages (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    provider_id TEXT NOT NULL,
    thread_id TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
    subject TEXT NOT NULL,
    sender TEXT NOT NULL,
    received_at INTEGER NOT NULL,
    body TEXT NOT NULL,
    snippet TEXT NOT NULL,
    UNIQUE(account_id, provider_id)
);
CREATE INDEX messages_received ON messages(received_at DESC, id);
CREATE INDEX messages_thread ON messages(thread_id, received_at, id);
CREATE TABLE raw_messages (
    message_id TEXT PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
    mime BLOB NOT NULL
);
CREATE TABLE labels (id TEXT PRIMARY KEY);
CREATE TABLE message_labels (
    message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    label_id TEXT NOT NULL REFERENCES labels(id),
    PRIMARY KEY(message_id, label_id)
);
CREATE INDEX message_labels_label ON message_labels(label_id, message_id);
CREATE TABLE attachments (
    message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    part_index INTEGER NOT NULL,
    name TEXT NOT NULL,
    content BLOB NOT NULL,
    PRIMARY KEY(message_id, part_index)
);
CREATE TABLE settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    theme TEXT NOT NULL CHECK (theme IN ('system', 'light', 'dark'))
);
INSERT INTO settings VALUES (1, 'system');
CREATE VIRTUAL TABLE messages_fts USING fts5(subject, sender, body, content='messages', content_rowid='rowid', tokenize='unicode61');
CREATE TRIGGER messages_insert AFTER INSERT ON messages BEGIN
    INSERT INTO messages_fts(rowid,subject,sender,body) VALUES(new.rowid,new.subject,new.sender,new.body);
END;
CREATE TRIGGER messages_delete AFTER DELETE ON messages BEGIN
    INSERT INTO messages_fts(messages_fts,rowid,subject,sender,body) VALUES('delete',old.rowid,old.subject,old.sender,old.body);
END;
CREATE TRIGGER messages_update AFTER UPDATE ON messages BEGIN
    INSERT INTO messages_fts(messages_fts,rowid,subject,sender,body) VALUES('delete',old.rowid,old.subject,old.sender,old.body);
    INSERT INTO messages_fts(rowid,subject,sender,body) VALUES(new.rowid,new.subject,new.sender,new.body);
END;
PRAGMA user_version = 1;
