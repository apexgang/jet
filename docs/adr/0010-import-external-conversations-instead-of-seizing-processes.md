# Import external conversations instead of seizing processes

Jet detects supported external Harness processes and reads their available native Conversation identity. V1 may show an external Conversation outside a registered Git Project as metadata, but managed Resume requires the user to register or map its repository so Jet can create or select a safe Workspace. Jet does not promise to seize arbitrary live processes or PTYs; live structured takeover is available only when the Harness exposes a cooperating endpoint.
