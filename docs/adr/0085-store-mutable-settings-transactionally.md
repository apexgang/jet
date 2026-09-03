# Store mutable settings transactionally

`~/.jet/config.toml` contains only bootstrap values needed before the store opens, such as data location, socket location, and diagnostic startup controls. Mutable settings live in SQLite and change through authenticated Commands. Resolution proceeds from built-in defaults through Plane, Project, and Conversation scopes, except where a setting is explicitly restricted to a narrower scope. A GUI may issue matching Commands to several connected Planes, but the results remain independently versioned and never form an atomic fleet-wide setting.
