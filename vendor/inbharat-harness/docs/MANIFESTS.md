# Capability and Tool Manifests

The JSON files in `examples/manifests/` document protocol-facing version 1. Runtime built-ins compile equivalent immutable `ToolManifest` values.

A tool manifest declares stable id/version, description, exact input/output JSON schemas, required capabilities, supported execution levels, determinism/idempotence, side-effect class, concurrency safety, confirmation mode, timeout, output limit, verification statement, and compensation statement.

Manifest text is descriptive; authority comes from the runtime `CapabilitySet`, deployment `PermissionProvider`, and `SandboxProvider`. Listing `file.write` in JSON never grants file writes. Dynamic model exposure is regenerated for every request from the intersection of manifest levels and granted capabilities.

Profile examples show secure headless and explicit local-development postures. They contain credential **references**, never values. Network destinations and allowed programs are empty by default. Runtime manifest loading is deliberately deferred; an embedding application must validate signatures/provenance before translating external JSON into trusted registrations.
