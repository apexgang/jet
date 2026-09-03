# Multiplex prioritized, credit-controlled streams

One GUI connection to a Plane multiplexes numbered Command, Query, Event, terminal, and Artifact streams. Control traffic has priority over bulk data, and each binary stream advances only within receiver-issued byte credit so terminal or Artifact traffic cannot exhaust the control allowance or grow memory without bound. Artifact completion additionally requires declared-size and SHA-256 verification.
