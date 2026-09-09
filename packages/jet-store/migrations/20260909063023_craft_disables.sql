CREATE TABLE craft_disables (
    craft_id TEXT PRIMARY KEY NOT NULL CHECK (length(craft_id) BETWEEN 1 AND 80),
    force INTEGER NOT NULL CHECK (force IN (0, 1))
);
