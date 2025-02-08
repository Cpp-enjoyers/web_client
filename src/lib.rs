/*!
    Web client module.

    `C++Enjoyers`: Implementation of the web client made by Simone Dagnino as individual contribution for AP 2024/2025

    The client autonomously sends flood requests and responses in order to build a graph
    of the network by following the WG protocol.

    The client can accept different command by the scl:
    * `AddSender(ID, Channel)`: adds a new direct neighbor to the client
    * `RemoveSender(ID)`: removes a direct neighbor from the client
    * `AskServerTypes`: asks to every server in the network their type
    * `AskListOfFiles(ID)`: asks to a specific server its own files
    * `RequestFile(Filename, ID)`: asks for a file from a specific server
    * `Shortcut(Packet)`: delivers to the client a packet that has been shortcut

    The client can send different events to the scl:
    * `PacketSent(Packet)`: logs that a packet has been sent over the network
    * `Shortcut(Packet)`: sends a packet that generated an error but cannot be lost
    * `ServersType(Types)`: sends the list with the type of each server in the network
    * `ListOfFiles(List, ID)`: sends the list of files of server with the given ID
    * `FileFromClient(FilesList, ID)`: sends a text file requested to the server with given ID and all the related media files
    * `UnsupportedRequest`: informs the client that it maded an invalid request

    The client must know the type of a server before asking for list of files or
    for a specific file: the simulation controller has to send an `AskServerTypes`
    command before any file-related command. After that, the scl can ask for a
    text file or for a text file list. It's client's job to download the
    requested file and to parse it, in order to download the correct media to
    send back to scl. If a text file doesn't contain any media link, it's
    directly forwared to scl; otherwise, it is stored in memory while waiting
    for media to be downloaded. This process involves a media file request to
    each media server and the following media file request if any of its file
    is needed from the client. When the whole content is ready, the scl receives
    the text files + each required media file.

*/

#![warn(clippy::pedantic)]
#![allow(clippy::cast_possible_truncation)]

pub(crate) mod utils;
pub mod web_client;
