This is a complete rewrite of xymon in rust.  

Several phases, first the client, independent of server, will send data with legacy compatibility to the legacy server. Common components, used by both client and server, go into the core crate.

The client crate lives in zymon/client, server in zymon/server and core goes into zymon/core.


Rules:
- fast, light, simple: performance is key
- no overcomplicating things, no unnecessary abstraction and indirection layers
- no adding entropy if not absolutely needed


The client:

The client has an init when first run to detect the system and check what's possible to monitor, then stores config for those tests: basic (memory, cpu, disk, raid, net, uptime, info, crontabs, dockers if supposed to stay running, ...), info also stores cpu type/model etc...  