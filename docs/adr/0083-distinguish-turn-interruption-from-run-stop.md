# Distinguish turn interruption from Run stop

Interrupt turn and Stop Run are separate Commands. Interruption prefers the Harness's native cancellation and keeps the Run active when the Harness supports it; stopping ends the entire Run. When native cancellation is unavailable, Jet escalates through interrupt, terminate, and kill, records forced termination distinctly, ends the Run when its Managed process dies, and preserves the Conversation, partial output, and Workspace changes for a later Run.
