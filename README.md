[![Linux Build](https://github.com/Niall-/boot/actions/workflows/build.yml/badge.svg)](https://github.com/Niall-/boot/actions/workflows/build.yml)

# boot

A common or garden IRC bot written in Rust, inspired by [url-bot-rs](https://github.com/nuxeh/url-bot-rs). Not fit for human consumption.

boot isn't currently suitable for end users as it's in need of some polish
and documentation, however it should be fairly stable and trivial to setup.
For now, if you would like to run boot please see the section for developers.

Please note that while there's nothing platform specific, boot is only tested on Linux.

Current features boot supports:
- Fetching titles from linked urls
- A seen command to show when a user was last seen speaking
- Notification/memo system
- Observed weather lookup with data provided by OpenWeatherMap
- Bitcoin/Ethereum price spark graphs/candles with data provided by Bitfinex

Planned features:
- Fetching metadata(e.g., image size/dimensions, video length, etc) from linked urls
- Conversion between common units of measurement
- A quote database
- Some common IRC games(IdleRPG, etc)

###### For developers

Current dependencies are SQLite, OpenSSL, and cURL. Adding features isn't too
complicated although it's a little unergonomic, argument parsing and dealing
with database read/writes are done in src/main.rs while features generally reside
in src/bot.rs. This is likely to change at some point although not by much.

To compile and run, simply compile with cargo as follows:
> cargo build --release
and run the binary in a folder with the necessary config file.

Pull requests are welcome, just please ensure that they compile and are stable.
