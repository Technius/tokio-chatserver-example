# Tokio Chat Server Example

This is a more involved example of using tokio to write a concurrent
application. This chat server demonstrates

* how to handling states in streams and futures
* how to use data structures that need to be shared between threads (e.g. to
  keep track of sessions)
* how to "broadcast" to streams
* how to shutdown a stream early

the server itself features

* private messages
* "commands"

## How to try it

First, open a terminal and compile with `cargo compile`. Then run `cargo run`.
Open two more terminals, and in each one, run `netcat localhost 8080`.

You can "log in" by typing a name and pressing <kbd>ENTER</kbd>. Then you can
send messages by entering some string and then hitting <kbd>ENTER</kbd>.

It's also possible to send a "private message" to user `<name>` by typing in
`/msg <name> <message>`.

## License

Copyright 2018 Bryan Tan ("Technius")

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
