pub(super) const SCHEMA_VERSION_V1: &str = "1";
pub(super) const SCHEMA_VERSION_V2: &str = "2";
pub(super) const SCHEMA_VERSION_V3: &str = "3";
pub(super) const SCHEMA_VERSION_V4: &str = "4";

pub const BLOB_CHUNK_SIZE: usize = 4 * 1024 * 1024;
pub const CHUNK_STORAGE_THRESHOLD: u64 = 16 * 1024 * 1024;

pub(super) const SCHEMA_V1: &str = r#"
CREATE TABLE vault_meta (
    key   TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
);

CREATE TABLE items (
    id          TEXT PRIMARY KEY NOT NULL,
    title       TEXT NOT NULL DEFAULT '',
    sha256      TEXT NOT NULL UNIQUE,
    mime_type   TEXT NOT NULL,
    width       INTEGER NOT NULL DEFAULT 0,
    height      INTEGER NOT NULL DEFAULT 0,
    byte_size   INTEGER NOT NULL,
    rating      INTEGER NOT NULL DEFAULT 0 CHECK (rating BETWEEN 0 AND 5),
    memo        TEXT NOT NULL DEFAULT '',
    palette     TEXT NOT NULL DEFAULT '[]',
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    deleted_at  INTEGER
);

CREATE INDEX idx_items_created_at ON items(created_at DESC) WHERE deleted_at IS NULL;
CREATE INDEX idx_items_rating ON items(rating DESC) WHERE deleted_at IS NULL;

CREATE TABLE thumbnails (
    item_id     TEXT PRIMARY KEY NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    mime_type   TEXT NOT NULL,
    width       INTEGER NOT NULL,
    height      INTEGER NOT NULL,
    data        BLOB NOT NULL
);

CREATE TABLE blobs (
    item_id     TEXT PRIMARY KEY NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    data        BLOB NOT NULL
);

CREATE TABLE tags (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL UNIQUE COLLATE NOCASE
);

CREATE TABLE item_tags (
    item_id     TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    tag_id      INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    PRIMARY KEY (item_id, tag_id)
);

CREATE INDEX idx_item_tags_tag_id ON item_tags(tag_id);
"#;

pub(super) const SCHEMA_V2: &str = r#"
CREATE TABLE IF NOT EXISTS groups (
    id          TEXT PRIMARY KEY NOT NULL,
    title       TEXT NOT NULL DEFAULT '',
    rating      INTEGER NOT NULL DEFAULT 0 CHECK (rating BETWEEN 0 AND 5),
    memo        TEXT NOT NULL DEFAULT '',
    reading_dir TEXT NOT NULL DEFAULT 'rtl' CHECK (reading_dir IN ('rtl', 'ltr')),
    spread_mode TEXT NOT NULL DEFAULT 'single' CHECK (spread_mode IN ('single', 'spread')),
    cover_alone INTEGER NOT NULL DEFAULT 1,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    deleted_at  INTEGER
);
CREATE INDEX IF NOT EXISTS idx_groups_created_at ON groups(created_at DESC) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_groups_rating ON groups(rating DESC) WHERE deleted_at IS NULL;

CREATE TABLE IF NOT EXISTS group_items (
    group_id    TEXT NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    item_id     TEXT NOT NULL UNIQUE REFERENCES items(id) ON DELETE CASCADE,
    page_order  INTEGER NOT NULL,
    PRIMARY KEY (group_id, item_id)
);
CREATE INDEX IF NOT EXISTS idx_group_items_order ON group_items(group_id, page_order ASC);

CREATE TABLE IF NOT EXISTS group_tags (
    group_id    TEXT NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    tag_id      INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    PRIMARY KEY (group_id, tag_id)
);
CREATE INDEX IF NOT EXISTS idx_group_tags_tag_id ON group_tags(tag_id);

CREATE TABLE IF NOT EXISTS folders (
    id          TEXT PRIMARY KEY NOT NULL,
    parent_id   TEXT REFERENCES folders(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    color       TEXT NOT NULL DEFAULT '',
    sort_order  INTEGER NOT NULL DEFAULT 0,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_folders_parent ON folders(parent_id, sort_order ASC, name COLLATE NOCASE ASC);

CREATE TABLE IF NOT EXISTS folder_items (
    folder_id   TEXT NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
    item_id     TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    added_at    INTEGER NOT NULL,
    PRIMARY KEY (folder_id, item_id)
);
CREATE INDEX IF NOT EXISTS idx_folder_items_item ON folder_items(item_id);

CREATE TABLE IF NOT EXISTS folder_groups (
    folder_id   TEXT NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
    group_id    TEXT NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    added_at    INTEGER NOT NULL,
    PRIMARY KEY (folder_id, group_id)
);
CREATE INDEX IF NOT EXISTS idx_folder_groups_group ON folder_groups(group_id);
"#;

pub(super) const SCHEMA_V3: &str = r#"
ALTER TABLE items ADD COLUMN duration_ms INTEGER NOT NULL DEFAULT 0;
ALTER TABLE items ADD COLUMN source_url TEXT NOT NULL DEFAULT '';
"#;

pub(super) const SCHEMA_V4: &str = r#"
ALTER TABLE items ADD COLUMN chunk_size INTEGER NOT NULL DEFAULT 0;
CREATE TABLE IF NOT EXISTS blob_chunks (
    item_id     TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    chunk_index INTEGER NOT NULL,
    data        BLOB NOT NULL,
    PRIMARY KEY (item_id, chunk_index)
);
"#;

pub(super) const PERFORMANCE_INDEXES: &str = r#"
CREATE INDEX IF NOT EXISTS idx_items_deleted_mime ON items(deleted_at, mime_type);
CREATE INDEX IF NOT EXISTS idx_items_deleted_byte_size ON items(deleted_at, byte_size);
CREATE INDEX IF NOT EXISTS idx_groups_deleted ON groups(deleted_at);
CREATE INDEX IF NOT EXISTS idx_group_items_group_order_item ON group_items(group_id, page_order ASC, item_id ASC);
"#;
