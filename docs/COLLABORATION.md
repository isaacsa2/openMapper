# Collaborative map editing

The intended experience is a shared workspace, similar to Roblox Studio's Team Create:
people build the same map together, see who is working where and keep their own undo
history without undoing somebody else's work.

## Available in this branch

File > Multiplayer hosts the open map or joins a host over LAN or a Cloudflare tunnel.
Roads, objects, terrain, painting, water, global settings and groups are synchronised.
Cursors, names, tools and selections show where the others are working. The host saves.
Everybody must already have the same objects and splines installed.

An edit is sent after its gesture ends. A remote tile is not installed while a brush or
drag is still held. Client queues are bounded; a full outgoing queue closes the connection
instead of blocking the window or silently discarding an edit. The local document stays
open after a disconnect and can be saved as a separate copy.

Conflicts are currently detected per tile (and per global settings/groups part). The host
accepts an edit only if all its base versions still match. Before an incoming state replaces
conflicting local work, that work is synced to a recovery file under the local map's
`openMapper/conflicts` folder. If that copy cannot be written, the session ends and the
local work is left open. After leaving, File > Multiplayer > Recover a conflicting edit
loads the copy into the current map as one undoable edit. Recovery does not save the map
or publish that edit back to the previous session.

## Remaining work toward the shared workspace

- Conflicts currently cover whole tiles: two people editing different objects on one tile
  can conflict. Object/property deltas and terrain-region ownership need a protocol change.
- Timetable, traffic, descriptions and chrono patches are not collaboratively edited yet.
- Connections have no automatic resume or offline edit replay. Rejoining starts from the
  host's current snapshot; keep a separate copy before reconnecting if local work matters.
- Asset transfer, view-only roles, owner/admin permissions, collaborator removal and
  persistent hosted projects are not implemented.
- A normal LAN session is not encrypted. Internet sessions use the Cloudflare tunnel.
- Actual UI behaviour on Windows/macOS and a real internet tunnel still need manual tests.

These are explicit limitations of this first implementation, not promises that the full
Team Create feature set is already available.
