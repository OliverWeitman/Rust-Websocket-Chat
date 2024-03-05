use futures_util::sink::SinkExt;
use futures_util::stream::{SplitSink, StreamExt};
use std::collections::HashMap;
use std::io::Error;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use tokio_tungstenite::{accept_async, tungstenite::protocol::Message, WebSocketStream};

type ChannelArc = Arc<Mutex<HashMap<String, broadcast::Sender<(String, SocketAddr)>>>>;
type SocketWriterSplit = SplitSink<WebSocketStream<TcpStream>, Message>;

type TungsteniteResult = Result<Message, tokio_tungstenite::tungstenite::Error>;

const ADDR: &str = "127.0.0.1:8080";

#[tokio::main]
async fn main() {
    let addr = ADDR;
    let server = TcpListener::bind(addr).await.unwrap();
    println!("Server running on {}", addr);

    let channels: ChannelArc = Arc::new(Mutex::new(HashMap::new()));

    //create a channel for all clients and add it to the channels hashmap
    let (tx, _rx) = broadcast::channel::<(String, SocketAddr)>(10);
    channels.lock().unwrap().insert("all".to_string(), tx);

    loop {
        let stream = server.accept().await;
        match stream {
            Ok((stream, _)) => {
                let channels = channels.clone(); // Clone the Arc so we can have it in the loop
                let start_channel = channels.lock().unwrap().get("all").unwrap().clone(); //Get the "all" channel from the hashmap

                tokio::spawn(async move {
                    connect(start_channel, stream, channels).await;
                });
            }
            Err(e) => {
                println!("Error: {}", e);
            }
        }
    }
}

async fn connect(
    start_channel: broadcast::Sender<(String, SocketAddr)>,
    stream: TcpStream,
    channels: ChannelArc,
) {
    // Accept the websocket connection
    let websocket: WebSocketStream<TcpStream> =
        accept_async(stream).await.expect("Handshake error");
    let (mut write, mut read) = websocket.split();

    let mut current_rx: broadcast::Receiver<(String, SocketAddr)> = start_channel.subscribe(); //subscribe to the "all" channel
    let mut current_channel: broadcast::Sender<(String, SocketAddr)> = start_channel.clone(); //set the current_channel to the "all" channel

    loop {
        tokio::select! {

            //ON MESSAGE SEND
            Some(message_result) = read.next() => { //receive message from the client
                let channel_change_option = handle_message_from_user(message_result, current_channel.clone(), channels.clone()).await;
                match channel_change_option {
                    Some(new_channel) => {

                        //if channel change actually worked
                        match new_channel {
                            Ok((new_channel, new_channel_key)) => {
                                current_rx = new_channel.subscribe(); //subscribe to the new_channel
                                current_channel = new_channel; //change the current_channel to the new_channel
                                current_channel.send((format!("Changed to channel to {}", new_channel_key), ADDR.parse().unwrap())).expect("Failed to send message to channel");
                            },
                            Err(e) => {
                                println!("Could not change channel: {}", e);
                                current_channel.send((format!("Could not change channel: {}", e), ADDR.parse().unwrap())).expect("Failed to send message to channel");
                            }
                        }

                    },
                    None => {}, //do nothing if the channel is not changed
                };
            },

            //ON MESSAGE RECEIVE
            Ok(msg) = current_rx.recv() => { //receive message from the broadcast channel
                get_channel_messages(msg.0, &mut write).await;
            },
        }
    }
}

async fn handle_message_from_user(
    message: TungsteniteResult,
    channel: broadcast::Sender<(String, SocketAddr)>,
    channels: ChannelArc,
) -> Option<Result<(broadcast::Sender<(String, SocketAddr)>, String), Error>> {
    let msg = match message {
        Ok(msg) => Some(msg.to_string()),
        Err(_e) => {
            None //handle error here
        }
    };
    let msg = msg.unwrap_or("".to_string()); //unwrap the message or set it to an empty string if it's None (WHICH IS AN ERROR AND SHOULD BE HANDLED!)

    if msg.contains(":create:") {
        //create a new channel
        let key = msg.replace(":create:", "").replace(":create: ", "");
        create_channel(&key, channels.clone()).await;
        Some(switch_channel(&key, channels));
    } else if msg.contains(":switch:") {
        //switch to a different channel
        let key = msg.replace(":switch:", "").replace(":switch: ", "");
        return Some(switch_channel(&key, channels).await); //return the new sender
    } else {
        println!("Message: {}", msg);
        channel
            .send((msg.to_string(), ADDR.parse().unwrap()))
            .expect("Failed to send message to channel");
    }
    return None;
}

//sending a message from the channel to the connected client
async fn get_channel_messages(msg: String, write: &mut SocketWriterSplit) {
    write.send(Message::Text(msg)).await.expect("Send error");
}

async fn create_channel(key: &String, channels: ChannelArc) -> bool {
    let (tx, _rx) = broadcast::channel::<(String, SocketAddr)>(10);
    let mut channels_guard = channels.lock().unwrap();
    channels_guard.insert(key.clone(), tx);
    println!("Channel created: {}", key);
    return true;
}

async fn switch_channel(
    key: &String,
    channels: ChannelArc,
) -> Result<(broadcast::Sender<(String, SocketAddr)>, String), std::io::Error> {
    let channels_guard = channels.lock().unwrap(); //get the channels hashmap
    let channel = channels_guard.get(key); //get the channel from the hashmap

    let channel = match channel {
        Some(channel) => channel.clone(),
        None => {
            return Err(Error::new(std::io::ErrorKind::Other, "Channel not found"));
        }
    };

    println!("Starting to change to channel: {}", key);
    return Ok((channel, key.to_string()));
}
