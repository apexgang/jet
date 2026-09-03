# Federate Plane-local full-text search

Each Plane incrementally indexes names, prompts, rendered responses, summaries, plans, file paths, and tool labels in a fast local full-text Search index. Raw terminal output, hidden native payloads, secrets, and artifact bodies are excluded by default. A GUI queries its connected Planes and merges their results, avoiding a central index, cloud dependency, or embedding pipeline in v1.
