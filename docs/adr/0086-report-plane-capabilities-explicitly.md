# Report Plane capabilities explicitly

`jetd` reports an explicit Capability snapshot at startup and on demand, covering the operating system, core and external-tool versions, credential availability, installed Crafts, supported Harnesses, and degraded conditions. It does not poll continuously, and every Command revalidates the capabilities it depends on before committing external work.
