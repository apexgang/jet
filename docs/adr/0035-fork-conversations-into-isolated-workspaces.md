# Fork Conversations into isolated Workspaces

A Conversation fork starts a new Conversation and isolated Workspace from a selected Change checkpoint, leaving the source unchanged. Jet uses the Harness's native fork when available and otherwise creates a new native conversation from a provenance-marked context package. Harness, account, execution mode, and placement are inherited as editable defaults, but source and fork never share one writable Workspace.
