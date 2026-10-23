-- Video bookmarks: named points in a recipe's video. Never edit after release.

CREATE TABLE video_bookmarks (
    id            INTEGER PRIMARY KEY,
    recipe_id     TEXT NOT NULL REFERENCES recipes(id) ON DELETE CASCADE,
    seconds       INTEGER NOT NULL CHECK (seconds >= 0),
    label         TEXT NOT NULL DEFAULT ''
);
CREATE INDEX video_bookmarks_recipe ON video_bookmarks(recipe_id, seconds);
