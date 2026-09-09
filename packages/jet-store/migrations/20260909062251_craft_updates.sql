-- Keep each accepted digest reachable while allowing successive installations.
ALTER TABLE craft_installation_plans RENAME TO previous_craft_installation_plans;
CREATE TABLE craft_installation_plans (
    effect_id TEXT PRIMARY KEY REFERENCES effects (effect_id),
    craft_id TEXT NOT NULL CHECK (length(craft_id) BETWEEN 1 AND 80),
    plan TEXT NOT NULL CHECK (length(plan) <= 262144)
);
INSERT INTO craft_installation_plans SELECT * FROM previous_craft_installation_plans;
DROP TABLE previous_craft_installation_plans;
