-- Friends, a local log of shared files, app settings, and who shared a recipe.
-- Never edit after release.

CREATE TABLE friends (
    id            INTEGER PRIMARY KEY,
    name          TEXT NOT NULL,
    method        TEXT NOT NULL CHECK (method IN ('messages', 'email')),
    address       TEXT NOT NULL,                      -- phone number or email
    created_at    TEXT NOT NULL
);

-- What was sent to or received from whom (local history only).
CREATE TABLE share_log (
    id            INTEGER PRIMARY KEY,
    friend_id     INTEGER REFERENCES friends(id) ON DELETE SET NULL,
    direction     TEXT NOT NULL CHECK (direction IN ('sent', 'received')),
    person        TEXT NOT NULL DEFAULT '',           -- sender's name, or the friend's at the time
    titles        TEXT NOT NULL DEFAULT '[]',         -- JSON array of recipe titles
    file_name     TEXT NOT NULL DEFAULT '',
    at            TEXT NOT NULL
);
CREATE INDEX share_log_friend ON share_log(friend_id, at);

CREATE TABLE settings (
    key           TEXT PRIMARY KEY NOT NULL,
    value         TEXT NOT NULL
);

ALTER TABLE recipes ADD COLUMN shared_by TEXT;       -- name of the friend it came from
