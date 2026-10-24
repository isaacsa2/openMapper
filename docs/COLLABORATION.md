# Multiplayer editing

The aim is a shared workspace, like Roblox Studio's Team Create: people build the same map
together, see who is working where and keep their own undo history without undoing somebody
else's work. This first version does part of that; what it does not do yet is listed at the
end.

File > Multiplayer… lets several people edit one map at once. One of them hosts the map open
in their openMapper; the others join with an address.

## Hosting and joining

* **On the same network:** the host ticks "Others on this network can join" and gives out one
  of the addresses the window lists (`192.168.0.10:27560`; port 27560 unless changed). The
  host's firewall must let the port in.
* **From anywhere:** the host ticks "Others anywhere can join". openMapper starts a free
  Cloudflare quick tunnel (openOMSI's own: `cloudflared` is fetched the first time and checked
  against its published SHA-256) and shows an `https://….trycloudflare.com` address. Nothing
  to set up on the router. The address changes every time.
* **Joining:** type or paste the address and the password. The host's map is copied into
  `maps/<folder>_multiplayer` and opened; a map of the same name you have is left alone.

Everybody needs the same objects and splines installed: only the map is sent, not the content
it uses.

## What is shared

As they are made: every tile (objects, roads and splines, ground, paint, water), the global
settings with the tile list, and the groups. Each edit goes to the host with the versions of
the tiles it changed; when somebody else changed one of them first, the edit is put back and
its author told. Undo stays each person's own and never puts back what somebody else's edit
replaced. Every editor hands out IDCodes from a block of its own. The others' cursors, names,
tools and selections are drawn in the view.

**Not shared yet:** the timetable, the traffic and AI and the descriptions. They stay the
host's: a guest's change of them is put back, with a message. **The host saves the map**; a
guest cannot save during the session, and can save their copy once they leave.

## Conflicts, held edits and recovery

Conflicts are found per tile (and per global settings and groups). The host takes an edit only
when every part of it is still at the version it was made from.

* **An edit is sent once its gesture ends.** A drag, a brush stroke or a road being drawn is
  one history step, open until it ends; while it is open, what the others did is read and
  held, and put in once it ends (put in its middle it would end the step while the tool still
  holds its starting state). Cursors, the list of people and the chat come in at once. At most
  4096 messages or 512 MiB are held this way; beyond that the session ends with a message and
  the map stays open.
* **Conflicting local work is kept.** Before the host's state replaces local work it
  conflicts with (an edit turned down, or a change not sent yet that somebody else's edit
  replaces), that work is written to a recovery file in the map's `openMapper/conflicts`
  folder (512 MiB at most). If it cannot be written, the session ends and the local work stays
  open. After leaving, File > Multiplayer > Recover a conflicting edit… puts a copy back into
  the open map as one undoable edit; it does not save the map nor send the edit to the session
  left. The folder is not emptied by itself.
* **Queues are bounded.** The editor sends one edit at a time; its cursor is not queued (only
  the latest goes). A send queue that still fills (a stalled connection) closes the connection
  rather than blocking the window or dropping an edit unseen; the map stays open and can be
  saved as a separate copy.

## Security: what to know before hosting

* **Anybody who joins can change the whole map**, everything listed above. There are no roles
  or read-only guests. Give the password only to people you would let edit the map.
* **A password is needed.** Hosting without one is a choice the window asks you to confirm,
  only possible on the local network, never through the tunnel. Over the internet the password
  must have at least 8 letters.
* **On the local network the connection is not encrypted.** Somebody who can listen in on it
  sees the map and its edits. The password itself is never sent: the host sends a fresh nonce
  with a salt and a work factor; the guest answers with an HMAC of the nonce under a key
  derived from the password with PBKDF2-HMAC-SHA256 (200,000 rounds). Somebody who catches a
  nonce and its answer can still try passwords on their own computer, though each try costs
  them the whole derivation: **a long password is what keeps it out of reach.** Use the tunnel
  when the network is not yours.
* **Through the tunnel** the connection is encrypted (HTTPS to Cloudflare, Cloudflare to the
  host's `cloudflared`). Cloudflare carries the traffic. The address is public: anybody who
  has it can try to join, so the password matters most here.

## Limits of a host

A host takes nothing unbounded from whoever reaches its port:

| What | Limit |
|---|---|
| People in a session | 16 (the host's own editor counted) |
| Connections still in their handshake | 8 at once; more are closed at once |
| A handshake | 10 seconds in all |
| Wrong passwords | checked one at a time; after each the next waits 250 ms, doubling up to 4 s (forgotten after a minute without one); a wrong one ends the connection |
| One message from a guest | 64 MiB (unpacked) |
| One edit | 4096 tiles and other parts |
| What the host holds | 512 MiB and 4096 parts more than the map it started with |
| Where new tiles may lie | within 32 tiles of the map's tiles at the start |
| A guest that stops reading | let go once 256 messages wait for it |
| A guest not heard from | gone after 45 seconds |

An edit beyond these is turned down whole and its author told why. Each edit sends whole tiles:
painting on tiles with many layers through the tunnel can be slow.

## Not done yet

* Conflicts cover whole tiles: two people editing different objects on one tile can conflict.
  Object and property changes, and terrain by region, need a change of the protocol.
* The timetable, the traffic, the descriptions and chrono patches are not edited together yet.
* A connection does not resume by itself and edits made offline are not replayed: joining
  again starts from the host's map as it is. Keep a separate copy first if local work matters.
* Sending the content (objects, splines, textures), read-only guests, roles, removing somebody
  from a session and projects that stay hosted are not there.
* How it behaves on Windows and macOS, and through a real tunnel, still needs testing by hand.
