# Anchor schedules to zoned deterministic firings

Every Scheduled task retains its original IANA time zone and derives a deterministic firing identity from the schedule and intended instant. A nonexistent daylight-saving local time advances to the next valid instant, and a repeated local time uses its first, earlier occurrence and fires once. Jet persists the selected UTC instant and firing identity so restart or time-zone database changes cannot duplicate it. When a Plane returns after being offline, Jet retains only the latest missed firing within seven days.
