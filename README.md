# web_client
C++Enjoyers - implementation of the web client made by Simone Dagnino (individual contribution)

The client allows for downloading html files and the relative images from web servers, accoring to the protocol. Furthermore, it features a seriel of logs that can be enabled by setting the environemt variable `RUST_LOG` to either `"error"` or `"info"`.

The documentation can be built by running
```shell
cargo doc --no-deps
```