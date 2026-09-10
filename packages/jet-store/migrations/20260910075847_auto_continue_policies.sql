CREATE TABLE auto_continue_policies (
    binding_id TEXT PRIMARY KEY NOT NULL REFERENCES account_bindings(binding_id) ON DELETE CASCADE,
    policy TEXT NOT NULL CHECK(json_valid(policy))
);
