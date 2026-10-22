-- Initial schema. Never edit after release; add a new migration instead.

CREATE TABLE recipes (
    id            TEXT PRIMARY KEY NOT NULL,          -- UUID v4
    title         TEXT NOT NULL,
    description   TEXT NOT NULL DEFAULT '',
    servings      INTEGER CHECK (servings > 0),
    prep_minutes  INTEGER CHECK (prep_minutes >= 0),
    cook_minutes  INTEGER CHECK (cook_minutes >= 0),
    photo_path    TEXT,                               -- relative to the media dir
    notes         TEXT NOT NULL DEFAULT '',
    rating        INTEGER CHECK (rating BETWEEN 1 AND 5),
    youtube_url   TEXT,
    video_path    TEXT,                               -- relative to the media dir
    video_status  TEXT NOT NULL DEFAULT 'none'
                  CHECK (video_status IN ('none', 'downloading', 'ready', 'failed', 'missing')),
    source_url    TEXT,
    calories      REAL,
    protein_g     REAL,
    carbs_g       REAL,
    fat_g         REAL,
    created_at    TEXT NOT NULL,                      -- RFC 3339 UTC
    updated_at    TEXT NOT NULL
);

-- A line is either an ingredient or a section heading (is_heading = 1, text in `item`).
CREATE TABLE ingredients (
    id            INTEGER PRIMARY KEY,
    recipe_id     TEXT NOT NULL REFERENCES recipes(id) ON DELETE CASCADE,
    position      INTEGER NOT NULL,
    is_heading    INTEGER NOT NULL DEFAULT 0 CHECK (is_heading IN (0, 1)),
    quantity      REAL,
    quantity_max  REAL,                               -- upper bound for ranges like "2-3"
    unit          TEXT,
    item          TEXT NOT NULL,
    prep_note     TEXT
);
CREATE INDEX ingredients_recipe ON ingredients(recipe_id, position);

CREATE TABLE steps (
    id            INTEGER PRIMARY KEY,
    recipe_id     TEXT NOT NULL REFERENCES recipes(id) ON DELETE CASCADE,
    position      INTEGER NOT NULL,
    text          TEXT NOT NULL
);
CREATE INDEX steps_recipe ON steps(recipe_id, position);

CREATE TABLE categories (
    id            INTEGER PRIMARY KEY,
    name          TEXT NOT NULL UNIQUE COLLATE NOCASE
);

CREATE TABLE tags (
    id            INTEGER PRIMARY KEY,
    name          TEXT NOT NULL UNIQUE COLLATE NOCASE
);

CREATE TABLE recipe_categories (
    recipe_id     TEXT NOT NULL REFERENCES recipes(id) ON DELETE CASCADE,
    category_id   INTEGER NOT NULL REFERENCES categories(id) ON DELETE CASCADE,
    PRIMARY KEY (recipe_id, category_id)
);
CREATE INDEX recipe_categories_category ON recipe_categories(category_id);

CREATE TABLE recipe_tags (
    recipe_id     TEXT NOT NULL REFERENCES recipes(id) ON DELETE CASCADE,
    tag_id        INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    PRIMARY KEY (recipe_id, tag_id)
);
CREATE INDEX recipe_tags_tag ON recipe_tags(tag_id);

-- Full-text index. Maintained by Rust in the same transaction as recipe writes.
CREATE VIRTUAL TABLE recipes_fts USING fts5(
    recipe_id UNINDEXED,
    title,
    description,
    ingredients,
    steps,
    notes,
    tokenize = 'unicode61 remove_diacritics 2'
);
