# Harness Schedule and Workspace runtime

This ESM bundle uses the published JavaScript from DeepSeek Harness 0.2.2-rc.1.
The source packages are `packages/schedule/schedule`, `packages/schedule/tool-schedule`,
`packages/workspace/workspace`, `packages/util/brand`, and `packages/util/util-time`
at upstream revision `fcc0ad46`. Their entry files are under
`dsh/node_modules` in the desktop app archive.

The bundle includes Temporal polyfill 0.5.1, RRule 2.8.1 and JSBI 4.3.2 from
the same archive. The ACP bridge binds Cordis, session, storage and the other
external imports to the running Host. The embedded runtime can therefore
use the same services and durable domain as the rest of the profile.
