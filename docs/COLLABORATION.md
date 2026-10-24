# Multiplayer editing

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
