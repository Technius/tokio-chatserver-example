use futures::{Future, Stream, Sink, future::Loop, sync::mpsc::UnboundedSender};
use std::net::SocketAddr;
use std::sync::{Arc, RwLock, atomic::{AtomicU64, Ordering}};
use std::collections::{HashMap, HashSet};

fn main() {
    let addr = "127.0.0.1:8080".parse::<SocketAddr>().unwrap();
    let listener = tokio::net::TcpListener::bind(&addr).expect("Could not bind socket");
    let mut runtime = tokio::runtime::Runtime::new()
        .expect("Could not start tokio runtime");

    let server_ctx = Arc::new(ServerContext::new());
    let server_ctx_clone = server_ctx.clone();
    let (control_tx, control_rx) = futures::sync::mpsc::unbounded::<(ClientId, Message)>();
    let keep_alive = control_tx.clone();

    // Handle incoming connections
    let server_fut = listener.incoming()
        .map_err(|_| ())
        .for_each(move |stream| {
            println!("Incoming connection");
            let (global_send, global_recv) = futures::sync::mpsc::unbounded::<Message>();
            let client_id = server_ctx_clone.add_client(global_send);

            let codec = tokio::codec::LinesCodec::new();
            let (output_frame, input_frame) = tokio::codec::Framed::new(stream, codec).split();

            // We handle the incoming messages as follows by forming an ad-hoc
            // stream that combines messages from various sources:
            // 1. Forward incoming strings to our new stream.
            // 2. Forward messages from the global dispatch channel into our new stream.
            // This will allow a client to handle messages in a unified fashion.
            // Also, note that we have to handle incoming messages indirecly so
            // that that the connection handler will be dropped when the client
            // disconnects.
            let (client_in_sender, client_in_stream) = futures::sync::mpsc::unbounded::<Message>();
            let input_fut = input_frame
                .for_each(move |line| {
                    client_in_sender.unbounded_send(Message::ClientInput(line)).unwrap();
                    futures::future::ok(())
                });
            let message_stream = client_in_stream.select(global_recv);

            // To simplify output handling, we create a `Sender` and a
            // `Receiver`. We pass the former to the client handler, and the
            // latter will forward any messages received to the client.
            let (client_out_sender, client_out_stream) = futures::sync::mpsc::unbounded::<String>();
            let output_fut = client_out_stream.map_err(|_| ())
                .forward(output_frame.sink_map_err(|_| ()))
                .map(|_| ());

            // Now, we form the future that will handle the client. The future
            // will complete (forcing the client to disconnect) in the when:
            // 1. The client closes its connection.
            // 2. The server closes the connection.
            let control_tx = control_tx.clone();
            let (control_fwd_send, control_fwd_recv) = futures::sync::mpsc::unbounded::<Message>();
            let fwd_fut = control_fwd_recv.map(move |msg| (client_id.clone(), msg))
                .map_err::<futures::sync::mpsc::SendError<_>, _>(|_| unreachable!())
                .forward(control_tx.clone());
            let server_ctx_clone = server_ctx_clone.clone();
            let final_fut = client_handle(message_stream, client_out_sender, control_fwd_send)
                .select2(input_fut)
                .select2(output_fut)
                .select2(fwd_fut)
                .inspect(move |_| {
                    println!("Connection terminated.");
                    let name_opt = server_ctx_clone.get_name(&client_id);
                    server_ctx_clone.terminate_client(&client_id);
                    if let Some(name) = name_opt {
                        let msg = (client_id, Message::UserLoggedOut { name: name.to_owned() });
                        control_tx.clone().unbounded_send(msg).unwrap();
                    }
                })
                .map(|_| ())
                .map_err(|_| ());

            // Lastly, create a new task that will handle the client.
            tokio::spawn(final_fut)
        })
        .map(|_| ());

    // Main message dispatcher; dispatches server-wide messages to clients
    let main_channel_fut = control_rx.for_each(move |(client_id, msg)| {
        match &msg {
            Message::UserLoggedIn { name } => {
                server_ctx.set_name(&client_id, name.clone());
            },
            _ => ()
        }
        server_ctx.clients.write().unwrap().retain(move |_, s| {
            if let Ok(_) = s.unbounded_send(msg.clone()) {
                true
            } else {
                false
            }
        });
        Ok(())
    });

    runtime.spawn(main_channel_fut);

    // Future to stop the server when a key is pressed
    let (shutdown_tx, shutdown_rx) = futures::sync::oneshot::channel::<()>();
    let shutdown_fut = shutdown_rx.map_err(|_| ());

    let final_fut = server_fut.select(shutdown_fut).map(|_| ()).map_err(|_| ());
    runtime.spawn(final_fut);

    println!("Server started, press enter to stop");
    let mut trash = String::new();
    std::io::stdin().read_line(&mut trash).unwrap();
    drop(keep_alive);
    shutdown_tx.send(()).unwrap();

    runtime.shutdown_on_idle().wait().unwrap();
}

type ClientId = u64;
type RoomId = u64;

#[derive(Debug)]
struct ServerContext {
    pub clients: Arc<RwLock<HashMap<ClientId, UnboundedSender<Message>>>>,
    pub sessions: Arc<RwLock<HashMap<ClientId, String>>>,
    pub rooms: Arc<RwLock<HashMap<RoomId, HashSet<ClientId>>>>,
    pub next_id: AtomicU64,
}

impl ServerContext {
    pub fn new() -> Self {
        ServerContext {
            clients: Arc::new(RwLock::new(HashMap::new())),
            sessions: Arc::new(RwLock::new(HashMap::new())),
            rooms: Arc::new(RwLock::new(HashMap::new())),
            next_id: AtomicU64::new(0),
        }
    }

    /// Initializes the client in this ServerContext.
    /// This will lock (write) the clients field.
    pub fn add_client(&self, sender: UnboundedSender<Message>) -> ClientId {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.clients.write().unwrap().insert(id, sender);
        id
    }

    /// Removes the client in this ServerContext.
    /// This will lock (write) every field.
    pub fn terminate_client(&self, id: &ClientId) {
        self.clients.write().unwrap().remove(id);
        self.sessions.write().unwrap().remove(id);
        self.rooms.write().unwrap().entry(*id)
            .and_modify(|clients| { clients.remove(id); () });
    }

    /// Sets the name of the given client.
    /// This will lock (write) the sessions field.
    pub fn set_name(&self, id: &ClientId, name: String) {
        self.sessions.write().unwrap().insert(*id, name);
    }

    /// Returns the name of the given client.
    /// This will lock (read) the sessions field.
    pub fn get_name(&self, id: &ClientId) -> Option<String> {
        self.sessions.read().unwrap().get(id).map(String::clone)
    }
}

#[derive(Debug, Clone)]
enum ClientState {
    AwaitingLogin,
    LoggedIn { name: String },
}

#[derive(Debug, Clone)]
enum Message {
    ClientInput(String),
    UserLoggedIn { name: String },
    UserLoggedOut { name: String },
    UserMessage { name: String, message: String }
}

fn client_handle<S>(_stream: S, output: UnboundedSender<String>,
                       main_channel: UnboundedSender<Message>)
                    -> impl Future<Item = (), Error = S::Error> where S : Stream<Item = Message> {
    // Handle the client using a state machine. We'll read from the stream, one
    // input at a time, until we decide to terminate the connection.
    futures::future::loop_fn((ClientState::AwaitingLogin, _stream), move |(state, stream)| {
        let main_channel = main_channel.clone();
        let output = output.clone();
        stream.into_future()
            .map(move |(msg_opt, cont)| {
                msg_opt
                    .and_then(move |msg| transition(state, msg, main_channel, output))
                    .map(move |st| Loop::Continue((st, cont)))
                    .unwrap_or(Loop::Break(()))
            })
    }).map_err(|(err, _)| err)
}

fn transition(state: ClientState, msg: Message, main_channel: UnboundedSender<Message>,
              output: UnboundedSender<String>) -> Option<ClientState> {
    match (&state, &msg) {
        (ClientState::AwaitingLogin, Message::ClientInput(name)) => {
            main_channel.unbounded_send(Message::UserLoggedIn { name: name.to_owned() }).unwrap();
            Some(ClientState::LoggedIn { name: name.to_owned() })
        },
        (ClientState::LoggedIn { name }, Message::ClientInput(message)) => {
            let msg = Message::UserMessage {
                name: name.to_owned(),
                message: message.to_owned()
            };
            main_channel.unbounded_send(msg).unwrap();
            Some(state)
        },
        (ClientState::LoggedIn { .. }, Message::UserMessage { name, message }) => {
            output.unbounded_send(format!("{}: {}", name, message)).unwrap();
            Some(state)
        },
        (ClientState::LoggedIn { .. }, Message::UserLoggedIn { name: user }) => {
            output.unbounded_send(format!("[SYSTEM] User {} logged in.", user)).unwrap();
            Some(state)
        },
        (ClientState::LoggedIn { .. }, Message::UserLoggedOut { name: user }) => {
            output.unbounded_send(format!("[SYSTEM] User {} logged out.", user)).unwrap();
            Some(state)
        },
        _ => Some(state)
    }
}
