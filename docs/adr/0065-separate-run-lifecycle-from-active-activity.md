# Separate Run lifecycle from active activity

A Run lifecycle progresses through `created`, `starting`, `active`, `stopping`, and exactly one terminal result: `completed`, `failed`, `canceled`, or `lost`. An active Run separately reports `working`, `waiting_for_user`, `waiting_for_approval`, `waiting_for_auth`, `waiting_for_quota`, or `reconnecting`. Keeping activity orthogonal avoids a growing cross-product of lifecycle states while retaining precise GUI and scheduling behavior.
