# Keep scheduled work in one Conversation

A Scheduled task belongs to one durable Conversation, and every firing submits another turn to that Conversation rather than creating a new one. If its Run is busy or its Plane is offline, `jetd` retains only the newest pending firing and submits it after the Run becomes available or the Plane returns, with a seven-day catch-up limit. Enabled Scheduled tasks protect their Conversation and Workspace from autodelete, and all firing outcomes remain in the Event journal.
