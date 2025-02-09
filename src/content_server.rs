use std::collections::{HashMap, HashSet};
use std::fs;
use std::time::Duration;
use crossbeam_channel::{select_biased, unbounded, Receiver, Sender};
use dronegowski_utils::functions::{assembler, fragment_message, generate_unique_id};
use dronegowski_utils::hosts::{ClientMessages, FileContent, ServerCommand, ServerEvent, ServerMessages, ServerType, TestMessage};
use log::log;
use wg_2024::network::{NodeId, SourceRoutingHeader};
use wg_2024::packet::{FloodRequest, FloodResponse, Fragment, NodeType, Packet, PacketType};
use crate::DronegowskiServer;
use serde::{Deserialize, Serialize};
use serde::de::DeserializeOwned;
use serde_json::Value;

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct TextServer {
    pub stored_texts: HashMap<u64, FileContent>,
}
impl TextServer {
    pub fn new() -> TextServer {
        Self{
            stored_texts: HashMap::new(),
        }
    }

    pub fn new_from_folder(folder_path: &str) -> Self {
        let mut stored_texts = HashMap::new();

        if let Ok(entries) = fs::read_dir(folder_path) {
            for entry in entries.flatten() {
                if let Ok(metadata) = entry.metadata() {
                    if metadata.is_file() {
                        if let Some(extension) = entry.path().extension() {
                            if extension == "json" { // Ora legge JSON invece di TXT
                                if let Ok(content) = fs::read_to_string(entry.path()) {
                                    if let Ok(json) = serde_json::from_str::<Value>(&content) {
                                        let id = stored_texts.len() as u64;
                                        let title = json["title"].as_str().unwrap_or("Untitled").to_string();
                                        let text = json["text"].as_str().unwrap_or("").to_string();

                                        let mut media_ids = Vec::new();
                                        if let Some(media_array) = json["media_ids"].as_array() {
                                            for media in media_array {
                                                if let (Some(id), Some(name)) = (media[0].as_u64(), media[1].as_str()) {
                                                    media_ids.push((id, name.to_string()));
                                                }
                                            }
                                        }

                                        stored_texts.insert(id, FileContent { title, text, media_ids });
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        Self { stored_texts }
    }

    pub fn list_files(&self) -> Vec<(u64, String)> {
        self.stored_texts.iter()
            .map(|(&id, file)| (id, file.title.clone()))
            .collect()
    }
    pub fn get_file_text(&self, file_id: u64) -> Option<FileContent> {
        self.stored_texts.get(&file_id).cloned()
    }
}


#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct MediaServer {
    pub stored_media: HashMap<u64, Vec<u8>>, // ID → Media
}
impl MediaServer {
    pub fn new() -> MediaServer {
        Self{
            stored_media: HashMap::new(),
        }
    }

    pub fn new_from_folder(folder_path: &str) -> Self {
        let mut stored_media = HashMap::new();

        if let Ok(entries) = fs::read_dir(folder_path) {
            for entry in entries.flatten() {
                if let Ok(metadata) = entry.metadata() {
                    if metadata.is_file() {
                        let id = stored_media.len() as u64;
                        if let Ok(content) = fs::read(entry.path()) {
                            stored_media.insert(id, content);
                        }
                    }
                }
            }
        }

        Self { stored_media }
    }

    pub fn get_media(&self, media_id: u64) -> Option<Vec<u8>> {
        self.stored_media.get(&media_id).cloned()
    }
}


pub struct ContentServer {
    id: NodeId,
    sim_controller_send: Sender<ServerEvent>,           //Channel used to send commands to the SC
    sim_controller_recv: Receiver<ServerCommand>,       //Channel used to receive commands from the SC
    packet_send: HashMap<NodeId, Sender<Packet>>,       //Map containing the sending channels of neighbour nodes
    packet_recv: Receiver<Packet>,                      //Channel used to receive packets from nodes
    server_type: ServerType,
    topology: HashSet<(NodeId, NodeId)>,                // Edges of the graph
    node_types: HashMap<NodeId, NodeType>,              // Node types (Client, Drone, Server)
    message_storage: HashMap<u64, Vec<Fragment>>,       // Store for reassembling messages
    text: TextServer,
    media: MediaServer,
}

impl DronegowskiServer for ContentServer {
    fn run(&mut self) {
        loop {
            select_biased! {
                recv(self.packet_recv) -> packet_res => {
                    log::info!("ContentServer {}: Received packet {:?}", self.id, packet_res);
                    if let Ok(packet) = packet_res {
                        self.handle_packet(packet);
                    }
                },
                recv(self.sim_controller_recv) -> command_res => {
                    log::info!("ContentServer {}: Received command {:?}", self.id, command_res);
                    if let Ok(command) = command_res {
                        self.handle_command(command);
                    }
                },
            }
        }
    }

    // packet receiving
    fn handle_packet(&mut self, packet: Packet){
        log::info!("ContentServer {}: Received packet {:?}", self.id, packet);
        if let Some(source_id) = packet.routing_header.source(){
            let key = packet.session_id;
            match packet.pack_type {
                PacketType::MsgFragment(fragment) => {
                    self.message_storage
                        .entry(key)
                        .or_insert_with(Vec::new)
                        .push(fragment.clone());
                    // Check if all the fragments are received
                    if let Some(fragments) = self.message_storage.get(&key) {
                        if let Some(first_fragment) = fragments.first() {
                            if fragments.len() as u64 == first_fragment.total_n_fragments {
                                match self.reconstruct_message(key) {
                                    Ok(message) => {
                                        if let TestMessage::WebServerMessages(client_messages) = message {
                                            match client_messages {
                                                ClientMessages::ServerType =>{
                                                    log::info!("ContentServer {}: Received server type request from {}", self.id, source_id);
                                                    self.send_message(ServerMessages::ServerType(ServerType::Content), source_id);
                                                },
                                                ClientMessages::FilesList =>{
                                                    log::info!("ContentServer {}: Received FilesList request from {}", self.id, source_id);
                                                    let list = self.text.list_files();
                                                    log::info!("ContentServer {}: sending FilesList to {}", self.id, source_id);
                                                    self.send_message(ServerMessages::FilesList(list), source_id);
                                                },
                                                ClientMessages::File(file_id) =>{
                                                    log::info!("ContentServer {}: Received File request (file_id {}) from {}", self.id, file_id, source_id);
                                                    match self.text.get_file_text(file_id) {
                                                        Some(text) => {
                                                            log::info!("ContentServer {}: sending file (file_id {}) to {}", self.id, file_id, source_id);
                                                            self.send_message(ServerMessages::File(text), source_id);
                                                        },
                                                        None => {
                                                            log::info!("ContentServer {}: file not found, sending error to {}", self.id, source_id);
                                                            self.send_message(ServerMessages::Error("file not found".to_string()),source_id);
                                                        }
                                                    }
                                                },
                                                ClientMessages::Media(media_id) =>{
                                                    log::info!("ContentServer {}: Received Media request (media_id {}) from {}", self.id, media_id, source_id);
                                                    match self.media.get_media(media_id) {
                                                        Some(media) => {
                                                            log::info!("ContentServer {}: sending media (media_id {}) to {}", self.id, media_id, source_id);
                                                            self.send_message(ServerMessages::Media(media), source_id);
                                                        },
                                                        None => {
                                                            log::info!("ContentServer {}: media not found, sending error to {}", self.id, source_id);
                                                            self.send_message(ServerMessages::Error("media not found".to_string()),source_id)
                                                        },
                                                    }
                                                },
                                                _ => {log::error!("ContentServer {}: Unkown message type", self.id);},
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        log::error!("ContentServer {}: Error reconstructing the message: {}", self.id, e);
                                    }
                                }
                            }
                        }
                    }
                }
                PacketType::FloodResponse(flood_response) => {
                    log::info!(
                    "ContentServer {}: Received FloodResponse: {:?}", self.id, flood_response);
                    // update the graph knowledge based on the new FloodResponse
                    self.update_graph(flood_response.path_trace);
                }
                PacketType::FloodRequest(flood_request) => {
                    log::info!("ContentServer {}: Received FloodRequest: {:?}", self.id, flood_request);

                    let mut response_path_trace = flood_request.path_trace.clone();
                    response_path_trace.push((self.id, NodeType::Server));

                    let flood_response = FloodResponse {
                        flood_id: flood_request.flood_id,
                        path_trace: response_path_trace,
                    };

                    let response_packet = Packet {
                        pack_type: PacketType::FloodResponse(flood_response),
                        routing_header: SourceRoutingHeader {
                            hop_index: 0,
                            hops: flood_request.path_trace.iter().rev().map(|(id, _)| *id).collect(),
                        },
                        session_id: packet.session_id,
                    };

                    log::info!("ContentServer {}: Sending FloodResponse: {:?}", self.id, response_packet);
                    let next_node = response_packet.routing_header.hops[0];
                    if let Some(sender) = self.packet_send.get(&next_node) {
                        match sender.send_timeout(response_packet.clone(), Duration::from_millis(500)) {
                            Err(_) => {
                                log::warn!("ContentServer {}: Timeout sending packet to {}", self.id, next_node);
                            }
                            Ok(..)=>{
                                log::info!("ContentServer {}: Sent FloodResponse back to {}", self.id, next_node);
                            }
                        }
                    } else {
                        log::warn!("ContentServer {}: No sender found for node {}", self.id, next_node);
                    }
                }
                PacketType::Ack(ack) => {
                    log::info!("ContentServer {}: Received Ack {:?} from {}", self.id, ack, source_id);
                }
                PacketType::Nack(nack) => {
                    log::info!("ContentServer {}: Received Nack {:?} from {}", self.id, nack, source_id);
                }
            }
        }
    }
    fn reconstruct_message<T: DeserializeOwned>(&mut self, key: u64) -> Result<T, Box<dyn std::error::Error>> {
        // Identifica il vettore di frammenti associato alla chiave
        if let Some(fragments) = self.message_storage.clone().get(&key) {
            if let Some(first_fragment) = fragments.first() {
                if fragments.len() as u64 == first_fragment.total_n_fragments {
                    self.message_storage.remove(&key);
                    // Crea una mappa indicizzata per ordinare i frammenti
                    let mut fragment_map: HashMap<u64, &Fragment> = HashMap::new();
                    for fragment in fragments {
                        fragment_map.insert(fragment.fragment_index, fragment);
                    }

                    // Inizializza il buffer per i dati completi
                    let mut full_data = Vec::new();

                    // Usa l'assembler per ciascun frammento in ordine
                    for index in 0..fragments.len() as u64 {
                        if let Some(fragment) = fragment_map.get(&index) {
                            assembler(&mut full_data, fragment);
                        } else {
                            return Err(format!("Frammento mancante con indice: {}", index).into());
                        }
                    }

                    // Deserializza il messaggio completo
                    let message: T = bincode::deserialize(&full_data)?;
                    Ok(message)
                } else {
                    Err(format!(
                        "Il numero totale di frammenti ({}) non corrisponde alla lunghezza del vettore ({})",
                        first_fragment.total_n_fragments,
                        fragments.len()
                    )
                        .into())
                }
            } else {
                Err("Nessun frammento trovato nella lista".into())
            }
        } else {
            Err(format!("Nessun frammento trovato per la chiave: {}", key).into())
        }
    }


    // message sending methods
    fn compute_best_path(&self, target_client: NodeId) -> Option<Vec<NodeId>> {
        use std::collections::VecDeque;

        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        let mut predecessors = HashMap::new();

        queue.push_back(self.id);
        visited.insert(self.id);

        while let Some(current) = queue.pop_front() {
            if current == target_client {
                let mut path = vec![current];
                while let Some(&pred) = predecessors.get(&path[0]) {
                    path.insert(0, pred);
                }
                return Some(path);
            }

            for &(node_a, node_b) in &self.topology {
                if node_a == current && !visited.contains(&node_b) {
                    visited.insert(node_b);
                    queue.push_back(node_b);
                    predecessors.insert(node_b, current);
                }
            }
        }

        None
    }


    // sc commands methods
    fn handle_command(&mut self, command: ServerCommand) {
        match command {
            ServerCommand::AddSender(id, sender) => {
                log::info!("ContentServer {}: Received AddSender Command: add {}", self.id, id);
                self.add_neighbor(id, sender);
            }
            ServerCommand::RemoveSender(id) => {
                log::info!("ContentServer {}: Received RemoveSender Command: remove {}", self.id, id);
                self.remove_neighbor(id);
            }
            _ =>{
                log::error!("ContentServer {}: Received unhandled ServerCommand type", self.id);
            }
        }
    }
    fn add_neighbor(&mut self, node_id: NodeId, sender: Sender<Packet>) {
        if let std::collections::hash_map::Entry::Vacant(e) = self.packet_send.entry(node_id) {
            e.insert(sender);
            log::info!("ContentServer {}: Successfully added {}", self.id, node_id);
            log::info!("ContentServer {}: starting a new network discovery", self.id);
            self.network_discovery();

        } else {
            log::error!("ContentServer {}: Sender for node {node_id} already stored in the map!", self.id);
        }
    }
    fn remove_neighbor(&mut self, node_id: NodeId) {
        if self.packet_send.contains_key(&node_id) {
            self.packet_send.remove(&node_id);
            self.remove_from_topology(node_id);
            log::info!("ContentServer {}: Successfully removed neighbour {}", self.id, node_id);
            log::info!("ContentServer {}: starting a new network discovery", self.id);
            self.network_discovery();
        } else {
            log::error!("ContentServer {}: the {} is not a neighbour", self.id, node_id);
        }
    }
    fn remove_from_topology(&mut self, node_id: NodeId) {
        self.topology.retain(|&(a, b)| a != node_id && b != node_id);
        self.node_types.remove(&node_id);
    }


    // network discovery related methods
    fn network_discovery(&self) {
        log::info!("ContentServer {}: starting Network discovery", self.id);
        let mut path_trace = Vec::new();
        path_trace.push((self.id, NodeType::Server));

        // Send flood_request to the neighbour nodes
        let flood_request = FloodRequest {
            flood_id: generate_unique_id(),
            initiator_id: self.id,
            path_trace,
        };

        for (node_id, sender) in &self.packet_send {
            log::info!("ContentServer {}: sending flood request to {}", self.id, node_id);
            let _ = sender.send(Packet {
                pack_type: PacketType::FloodRequest(flood_request.clone()),
                routing_header: SourceRoutingHeader {
                    hop_index: 0,
                    hops: vec![self.id, *node_id],
                },
                session_id: flood_request.flood_id,
            });
        }
    }
    fn update_graph(&mut self, path_trace: Vec<(NodeId, NodeType)>) {
        log::info!("ContentServer {}: updating graph knowledge using: {:?}", self.id, path_trace);
        for i in 0..path_trace.len() - 1 {
            let (node_a, _) = path_trace[i];
            let (node_b, _) = path_trace[i + 1];
            self.topology.insert((node_a, node_b));
            self.topology.insert((node_b, node_a)); // Grafo bidirezionale
        }

        for (node_id, node_type) in path_trace {
            self.node_types.insert(node_id, node_type);
        }
        log::info!("ContentServer {}: topology after update: {:?}", self.id, self.topology);
    }

}

impl ContentServer {
    pub fn new(id: NodeId, sim_controller_send: Sender<ServerEvent>, sim_controller_recv: Receiver<ServerCommand>, packet_recv: Receiver<Packet>, packet_send: HashMap<NodeId, Sender<Packet>>, server_type: ServerType, file_path:&str, media_path:&str) -> Self {

        let mut server = Self {
            id,
            sim_controller_send,
            sim_controller_recv,
            packet_recv,
            packet_send,
            server_type,
            message_storage: HashMap::new(),
            topology: HashSet::new(),
            node_types: HashMap::new(),
            text: TextServer::new_from_folder(file_path),
            media: MediaServer::new_from_folder(media_path),

        };

        server.network_discovery();

        server
    }


    //message sending_method
    fn send_message(&mut self, message: ServerMessages, destination: NodeId) {
        log::info!("ContentServer {}: sending packet to {}", self.id, destination);
        let route=self.compute_best_path(destination).unwrap_or(Vec::new());
        log::info!("ContentServer {}: sending through route {:?}", self.id, route);
        if let Some(&neighbour_id) = route.get(1) {
            log::info!("ContentServer {}: sending packet to {}", self.id, neighbour_id);
            if let Some(sender) = self.packet_send.get(&neighbour_id) {

                let packets = fragment_message(&TestMessage::WebClientMessages(message), route, 1);

                for mut packet in packets {
                    packet.routing_header.hop_index = 1;
                    sender.send(packet).expect("Error occurred sending the message to the neighbour.");
                }
            } else {
                log::error!("ContentServer {}: Neighbour {} not found!", self.id, neighbour_id);
            }
        } else {
            log::error!("ContentServer {}: There is no available route", self.id);
        }
    }

}