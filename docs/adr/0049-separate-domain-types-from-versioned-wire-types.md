# Separate domain types from versioned wire types

Core domain types do not double as client, Craft, or fuel protocol objects. `jet-protocol` owns versioned Rust wire DTOs, emits machine-readable JSON schemas and generated Swift and TypeScript models, and uses the bounded framing established for the Jet protocol. `jetd` translates at the seam so backward compatibility does not freeze the core model and unknown Presentation blocks remain preservable.
