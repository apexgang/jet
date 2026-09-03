# Enter read-only Recovery mode on corruption

After an unclean shutdown `jetd` performs a lightweight integrity check, with deeper checks scheduled while idle. Failure enters Recovery mode: new Runs and mutations stop, the damaged database is preserved, and the user may diagnose, export, or explicitly restore the newest verified Recovery snapshot. Jet never replaces corrupt state with an empty store or overwrites the damaged copy silently; surviving `jetfueld` replay is applied only after authoritative restoration succeeds.
