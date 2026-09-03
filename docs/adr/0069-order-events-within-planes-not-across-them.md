# Order Events within Planes, not across them

The Event journal provides a total monotonic sequence only within one Plane. Wall-clock timestamps are display metadata and never a concurrency or causality authority across Planes. Federated GUI clients preserve each Plane's sequence and merge visible streams deterministically using timestamp, Plane identity, and local sequence, without claiming a globally authoritative event order.
