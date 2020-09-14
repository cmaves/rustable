# rustable
rustable is yet another library for interfacing Bluez over DBus.
Its objective is to be a comprehensive tool for creating Bluetooth Low Energy
enabled applications in Rust on Linux. It will supports to interacting with remote
devices as a GATT client, and creating local services as a GATT Server.
It currently allows the creation of advertisements/broadcasts as a Bluetooth peripherial.
//!
## Supported Features
### GAP Peripheral
- Advertisements
- Broadcasts
### GATT Server
- Creating local services
- Reading local characteristics from remote devices.
- Writing to local characteristics from remote devices.
- Write-without-response via sockets from remote devices (AcquireWrite).
- Notifying/Indicating local characteristics with sockets (AcquireNotify).
- Reading local descriptors from remote devices.
 **To Do:**
- Writable descriptors.
### GATT Client
- Retreiving attribute metadata (Flags, UUIDs...).
- Reading from remote characteristics.
- Writing to remote characteristics.
- Write-without-response via sockets to remote devices (AcquireWrite).
- Receiving remote notification/indications with sockets.
 **To Do:**
- Descriptors as a client.
## Development status
This library is unstable in *alpha*. There are planned functions
in the API that have yet to be implemented. Unimplemented function are noted.
The API is subject to breaking changes.
## Documentation
Documentation for the master branch can be found [here](https://rustable.maves.io/).

Documentation for the current release can be found [here](https://docs.rs/rustable/0.1.1/rustable)
