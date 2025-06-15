use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::time::Duration;
use crossbeam_channel::{select_biased, unbounded, Receiver, Sender};
use dronegowski_utils::functions::{assembler, fragment_message, generate_unique_id};
use dronegowski_utils::hosts::{ClientMessages, FileContent, ServerCommand, ServerEvent, ServerMessages, ServerType, TestMessage};
use log::{error, info, log, warn};
use wg_2024::network::{NodeId, SourceRoutingHeader};
use wg_2024::packet::{Ack, FloodRequest, FloodResponse, Fragment, Nack, NackType, NodeType, Packet, PacketType};
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
                            if extension == "json" {
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

    // returns a Vec with the ids and title of the files stored in the TextServer
    pub fn list_files(&self) -> Vec<(u64, String)> {
        self.stored_texts.iter()
            .map(|(&id, file)| (id, file.title.clone()))
            .collect()
    }

    //returns a file content with requested id, if there is one otherwise None
    pub fn get_file_text(&self, file_id: u64) -> Option<FileContent> {
        self.stored_texts.get(&file_id).cloned()
    }
}


#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct MediaServer {
    pub stored_media: HashMap<u64, Vec<u8>>,
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
                        if let Some(file_name) = entry.file_name().to_str() {
                            if let Some(first_digit) = file_name.chars().next().and_then(|c| c.to_digit(10)) {
                                let id = first_digit as u64;
                                if let Ok(content) = fs::read(entry.path()) {
                                    stored_media.insert(id, content);
                                }
                            }
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
    sim_controller_send: Sender<ServerEvent>,           // Channel used to send commands to the SC
    sim_controller_recv: Receiver<ServerCommand>,       // Channel used to receive commands from the SC
    packet_send: HashMap<NodeId, Sender<Packet>>,       // Map containing the sending channels of neighbour nodes
    packet_recv: Receiver<Packet>,                      // Channel used to receive packets from nodes

    server_type: ServerType,                            // Server Type

    topology: HashSet<(NodeId, NodeId)>,                // Edges of the graph
    node_types: HashMap<NodeId, NodeType>,              // Node types (Client, Drone, Server)

    message_storage: HashMap<u64, Vec<Fragment>>,       // Store for reassembling messages

    pending_messages: HashMap<u64, Vec<Packet>>,        // Storage of not acked fragments
    acked_fragments: HashMap<u64, HashSet<u64>>,        // storage of acked fragments
    nack_counter: HashMap<(u64, u64, NodeId), u64>,      // Counter used to manage when is better to use and alternative route
    excluded_nodes: HashSet<NodeId>,                    // ids of the drones that dropped enough fragments

    text: TextServer,
    media: MediaServer,
}

impl DronegowskiServer for ContentServer {
    fn run(&mut self) {
        loop {
            select_biased! {
                recv(self.packet_recv) -> packet_res => {
                    //log::info!("ContentServer {}: Received packet {:?}", self.id, packet_res);
                    if let Ok(packet) = packet_res {
                        self.handle_packet(packet);
                    }
                },
                recv(self.sim_controller_recv) -> command_res => {
                    //log::info!("ContentServer {}: Received command {:?}", self.id, command_res);
                    if let Ok(command) = command_res {
                        self.handle_command(command);
                    }
                },
            }
        }
    }

    // packet receiving
    fn handle_packet(&mut self, packet: Packet){
        //log::info!("ContentServer {}: Received packet {:?}", self.id, packet);
        if let Some(source_id) = packet.routing_header.source(){
            let key = packet.session_id;
            match packet.pack_type {
                PacketType::MsgFragment(ref fragment) => {

                    let _ = self
                        .sim_controller_send
                        .send(ServerEvent::DebugMessage(self.id, format!("Client {}: received from {}", self.id, source_id)));

                    // send an ack to the sender for this fragment
                    self.send_ack(packet.clone(), fragment.clone());

                    // store the fragment in order to reconstruct the message
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

                                            // Sends the received message to the simulation controller.
                                            let _send_sc = self
                                                .sim_controller_send
                                                .send(ServerEvent::MessageReceived(TestMessage::WebServerMessages(client_messages.clone())));

                                            // handles the message in relation of its type
                                            match client_messages {
                                                ClientMessages::ServerType =>{
                                                    log::info!("ContentServer {}: Received server type request from {}", self.id, source_id);
                                                    self.send_message(ServerMessages::ServerType(ServerType::Content), source_id);
                                                },
                                                ClientMessages::FilesList =>{
                                                    //log::info!("ContentServer {}: Received FilesList request from {}", self.id, source_id);
                                                    let list = self.text.list_files();
                                                    //log::info!("ContentServer {}: sending FilesList to {}", self.id, source_id);
                                                    self.send_message(ServerMessages::FilesList(list), source_id);
                                                },
                                                ClientMessages::File(file_id) =>{
                                                    //log::info!("ContentServer {}: Received File request (file_id {}) from {}", self.id, file_id, source_id);
                                                    match self.text.get_file_text(file_id) {
                                                        Some(text) => {
                                                            //log::info!("ContentServer {}: sending file (file_id {}) to {}", self.id, file_id, source_id);
                                                            self.send_message(ServerMessages::File(text), source_id);
                                                        },
                                                        None => {
                                                            //log::info!("ContentServer {}: file not found, sending error to {}", self.id, source_id);
                                                            self.send_message(ServerMessages::Error("file not found".to_string()),source_id);
                                                        }
                                                    }
                                                },
                                                ClientMessages::Media(media_id) =>{
                                                    //log::info!("ContentServer {}: Received Media request (media_id {}) from {}", self.id, media_id, source_id);
                                                    match self.media.get_media(media_id) {
                                                        Some(media) => {
                                                            //log::info!("ContentServer {}: sending media (media_id {}) to {}", self.id, media_id, source_id);
                                                            self.send_message(ServerMessages::Media(media), source_id);
                                                        },
                                                        None => {
                                                            //log::info!("ContentServer {}: media not found, sending error to {}", self.id, source_id);
                                                            self.send_message(ServerMessages::Error("media not found".to_string()),source_id)
                                                        },
                                                    }
                                                },
                                                _ => {
                                                    self.send_message(ServerMessages::Error("Unknown message type".to_string()), source_id);
                                                    log::error!("ContentServer {}: Unknown message type", self.id);
                                                },
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
                    log::info!("ContentServer {}: Received FloodResponse: {:?}", self.id, flood_response);

                    // update the graph knowledge based on the new FloodResponse
                    self.update_graph(flood_response.path_trace);
                }
                PacketType::FloodRequest(flood_request) => {
                    log::info!("ContentServer {}: Received FloodRequest: {:?}", self.id, flood_request);

                    // push myself in the path trace
                    let mut response_path_trace = flood_request.path_trace.clone();
                    response_path_trace.push((self.id, NodeType::Server));


                    // build the flood response packet.
                    let flood_response = FloodResponse {
                        flood_id: flood_request.flood_id,
                        path_trace: response_path_trace.clone(),
                    };
                    let response_packet = Packet {
                        pack_type: PacketType::FloodResponse(flood_response),
                        routing_header: SourceRoutingHeader {
                            hop_index: 1,
                            hops: response_path_trace.iter().rev().map(|(id, _)| *id).collect(),
                        },
                        session_id: packet.session_id,
                    };

                    info!("ContentServer {}: Sending FloodResponse: {:?}", self.id, response_packet);
                    // send the flood response
                    let next_node = response_packet.routing_header.hops[1];

                    self.send_packet_and_notify(response_packet.clone(), next_node);
                    // if let Some(sender) = self.packet_send.get(&next_node) {
                    //     match sender.send_timeout(response_packet.clone(), Duration::from_millis(500)) {
                    //         Err(_) => {
                    //             log::warn!("ContentServer {}: Timeout sending packet to {}", self.id, next_node);
                    //         }
                    //         Ok(..)=>{
                    //             //log::info!("ContentServer {}: Sent FloodResponse back to {}", self.id, next_node);
                    //             // notify the SC that I sent a flood_response
                    //             let _ = self
                    //                 .sim_controller_send
                    //                 .send(ServerEvent::PacketSent(response_packet.clone()));
                    //         }
                    //     }
                    // } else {
                    //     log::error!("ContentServer {}: No sender found for node {}", self.id, next_node);
                    // }
                }
                PacketType::Ack(ack) => {
                    //log::info!("ContentServer {}: Received Ack {:?} from {}", self.id, ack, source_id);
                    self.handle_ack(ack.clone(), packet.session_id);
                }
                PacketType::Nack(ref nack) => {
                    //log::info!("ContentServer {}: Received Nack {:?} from {}", self.id, nack, source_id);

                    // call the nack handler method passing it the drone which dropped
                    let drop_drone = packet.clone().routing_header.hops[0];
                    self.handle_nack(nack.clone(), packet.session_id, drop_drone);
                }
            }
        }
    }
    fn reconstruct_message<T: DeserializeOwned>(&mut self, key: u64) -> Result<T, Box<dyn std::error::Error>> {
        // identify the Vec of fragments associated with this specific key
        if let Some(fragments) = self.message_storage.clone().get(&key) {
            if let Some(first_fragment) = fragments.first() {
                if fragments.len() as u64 == first_fragment.total_n_fragments {
                    self.message_storage.remove(&key);
                    // sort the fragments using a map
                    let mut fragment_map: HashMap<u64, &Fragment> = HashMap::new();
                    for fragment in fragments {
                        fragment_map.insert(fragment.fragment_index, fragment);
                    }

                    let mut full_data = Vec::new();

                    // assemble the message
                    for index in 0..fragments.len() as u64 {
                        if let Some(fragment) = fragment_map.get(&index) {
                            assembler(&mut full_data, fragment);
                        } else {
                            return Err(format!("Missing fragment of index: {}", index).into());
                        }
                    }

                    // Deserialize the complete message
                    let message: T = bincode::deserialize(&full_data)?;
                    Ok(message)
                } else {
                    Err(format!(
                        "Total number of fragments ({}) does not correspond with the Vec length ({})",
                        first_fragment.total_n_fragments,
                        fragments.len()
                    )
                        .into())
                }
            } else {
                Err("No fragment found".into())
            }
        } else {
            Err(format!("No fragment found using key: {}", key).into())
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
                //log::info!("ContentServer {}: Received AddSender Command: add {}", self.id, id);
                self.add_neighbor(id, sender);
            }
            ServerCommand::RemoveSender(id) => {
                //log::info!("ContentServer {}: Received RemoveSender Command: remove {}", self.id, id);
                self.remove_neighbor(id);
            }
            ServerCommand::ControllerShortcut(packet)=>{
                //info!("ContentServer {}: Received ControllerShortcut", self.id);
                self.handle_packet(packet.clone());
            }
            ServerCommand::RequestNetworkDiscovery => {self.network_discovery()},
            _ =>{
                log::error!("ContentServer {}: Received unhandled ServerCommand type", self.id);
            }
        }
    }
    fn add_neighbor(&mut self, node_id: NodeId, sender: Sender<Packet>) {
        if let std::collections::hash_map::Entry::Vacant(e) = self.packet_send.entry(node_id) {
            e.insert(sender);
            //log::info!("ContentServer {}: Successfully added {}", self.id, node_id);
            //log::info!("ContentServer {}: starting a new network discovery", self.id);
            self.network_discovery();

        } else {
            log::error!("ContentServer {}: Sender for node {node_id} already stored in the map!", self.id);
        }
    }
    fn remove_neighbor(&mut self, node_id: NodeId) {
        if self.packet_send.contains_key(&node_id) {
            self.packet_send.remove(&node_id);
            self.remove_from_topology(node_id);
            //log::info!("ContentServer {}: Successfully removed neighbour {}", self.id, node_id);
            //log::info!("ContentServer {}: starting a new network discovery", self.id);
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
        //log::info!("ContentServer {}: starting Network discovery", self.id);
        let mut path_trace = Vec::new();
        path_trace.push((self.id, NodeType::Server));

        // Send flood_request to the neighbour nodes
        let flood_request = FloodRequest {
            flood_id: generate_unique_id(),
            initiator_id: self.id,
            path_trace,
        };

        for (node_id, sender) in &self.packet_send {
            //log::info!("ContentServer {}: sending flood request to {}", self.id, node_id);

            // build the flood_request packets
            let flood_request_packet = Packet {
                pack_type: PacketType::FloodRequest(flood_request.clone()),
                routing_header: SourceRoutingHeader {
                    hop_index: 0,
                    hops: vec![self.id, *node_id],
                },
                session_id: flood_request.flood_id,
            };

            // send the flood_request
            let _send_neighbor = sender.send(flood_request_packet.clone());

            // notify the SC that I sent a flood_request
            let _send_sc = self
                .sim_controller_send
                .send(ServerEvent::PacketSent(flood_request_packet.clone()));
        }
    }
    fn update_graph(&mut self, path_trace: Vec<(NodeId, NodeType)>) {
        log::info!("ContentServer {}: updating graph knowledge using: {:?}", self.id, path_trace);
        for i in 0..path_trace.len() - 1 {
            let (node_a, _node_type) = path_trace[i];
            let (node_b, _node_type) = path_trace[i + 1];
            self.topology.insert((node_a, node_b));
            self.topology.insert((node_b, node_a));
        }

        for (node_id, node_type) in path_trace {
            self.node_types.insert(node_id, node_type);
        }
        //log::info!("ContentServer {}: topology after update: {:?}", self.id, self.topology);
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

            pending_messages: HashMap::new(),
            nack_counter: HashMap::new(),
            excluded_nodes: HashSet::new(),
            acked_fragments: HashMap::new(),

            text: TextServer::new_from_folder(file_path),
            media: MediaServer::new_from_folder(media_path),

        };

        server.network_discovery();

        server
    }


    //message sending_methods
    fn send_message(&mut self, message: ServerMessages, destination: NodeId) {
        //log::info!("ContentServer {}: sending packet to {}", self.id, destination);
        let route=self.compute_best_path(destination).unwrap_or(Vec::new());
        //log::info!("ContentServer {}: sending through route {:?}", self.id, route);
        // sending route to SC
        let _ = self
            .sim_controller_send
            .send(ServerEvent::Route(route.clone()));

        if let Some(&neighbour_id) = route.get(1) {
            //log::info!("ContentServer {}: sending packet to {}", self.id, neighbour_id);
            if let Some(sender) = self.packet_send.get(&neighbour_id) {

                let session_id = generate_unique_id();

                let packets = fragment_message(&TestMessage::WebClientMessages(message), route, session_id);

                self.pending_messages.insert(session_id, packets.clone());

                for mut packet in packets {
                    packet.routing_header.hop_index = 1;
                    sender.send(packet.clone()).expect("Error occurred sending the message to the neighbour.");

                    // notify the SC that I sent a message
                    let _send_sc = self
                        .sim_controller_send
                        .send(ServerEvent::PacketSent(packet.clone()));
                }
            } else {
                log::error!("ContentServer {}: Neighbour {} not found!", self.id, neighbour_id);
            }
        } else {
            log::error!("ContentServer {}: There is no available route", self.id);
        }
    }
    fn send_ack(&mut self, packet: Packet, fragment: Fragment) {
        //log::info!("ContentServer {}: Sending Ack for fragment {}", self.id, fragment.fragment_index);

        let reversed_hops: Vec<NodeId> = packet.routing_header.hops.iter().rev().cloned().collect();
        let ack_routing_header = SourceRoutingHeader {
            hop_index: 1,
            hops: reversed_hops,
        };

        let ack_packet = Packet::new_ack(
            ack_routing_header,
            packet.session_id,
            fragment.fragment_index,
        );

        if let Some(next_hop) = ack_packet.routing_header.hops.get(1).cloned() {

            //log::info!("ContentServer {}: sending ack {:?} to {}", self.id, ack_packet, next_hop);
            if let Some(sender) = self.packet_send.get(&next_hop) {
                sender.send(ack_packet.clone()).expect("Error occurred sending the ack to the neighbour.");
                let _send_sc = self
                    .sim_controller_send
                    .send(ServerEvent::PacketSent(ack_packet.clone()));
            } else {
                log::error!("ContentServer {}: Neighbour {} not found!", self.id, next_hop);
            }

        } else {
            log::warn!("ContentServer {}: No valid path to send Ack for fragment {}", self.id, fragment.fragment_index);
        }

    }
    fn send_packet_and_notify(&self, packet: Packet, recipient_id: NodeId) {
        if let Some(sender) = self.packet_send.get(&recipient_id) {
            if let Err(e) = sender.send(packet.clone()) {
                error!(
                    "ContentServer {}: Error sending packet to {}: {:?}",
                    self.id,
                    recipient_id,
                    e
                );
            } else {
                // info!(
                //     "ContentServer {}: Packet sent to {}: must arrive at {}",
                //     self.id,
                //     recipient_id,
                //     packet.routing_header.hops.last().unwrap(),
                // );

                // Notifies the SC of packet sending.
                let _send_sc = self
                    .sim_controller_send
                    .send(ServerEvent::PacketSent(packet));
            }
        } else {
            error!("ContentServer {}: No sender for node {}", self.id, recipient_id);
        }
    }

    // ack handling
    fn handle_ack(&mut self, ack: Ack, session_id: u64) {
        let fragment_index = ack.fragment_index;

        // Removes from ack_counter if there is this packet
        self.nack_counter.retain(|(f_idx, s_id, _), _| !(*f_idx == fragment_index && *s_id == session_id));

        // updates acked_fragments
        let acked = self.acked_fragments.entry(session_id).or_default();
        acked.insert(fragment_index);

        // Check whether all the fragments of this session have been acked
        if let Some(fragments) = self.pending_messages.get(&session_id) {
            let total_fragments = fragments.len() as u64;
            if acked.len() as u64 == total_fragments {
                self.pending_messages.remove(&session_id);
                self.acked_fragments.remove(&session_id);
                //info!("ContentServer {}: All fragments for session {} have been acknowledged", self.id, session_id);
            }
        }
    }


    // nack handling
    fn handle_nack(&mut self, nack: Nack, session_id: u64, id_drop_drone: NodeId) {
        let key = (nack.fragment_index, session_id, id_drop_drone);

        // Uses Entry to correctly handle counter initialization
        let counter = self.nack_counter.entry(key).or_insert(0);
        *counter += 1;

        match nack.nack_type {
            NackType::Dropped => {
                if *counter == 4 {
                    //info!("Client {}: Too many NACKs for fragment {}. Calculating alternative path", self.id, nack.fragment_index);
                    let _ = self
                        .sim_controller_send
                        .send(ServerEvent::DebugMessage(self.id, format!("Server {}: nack drop {} from {} / {}", self.id, counter, id_drop_drone, nack.fragment_index)));

                    // Add the problematic node to excluded nodes
                    self.excluded_nodes.insert(id_drop_drone);

                    let _ = self
                        .sim_controller_send
                        .send(ServerEvent::DebugMessage(self.id, format!("Server {}: new route exclude {:?}", self.id, self.excluded_nodes)));

                    self.network_discovery();

                    // Reconstruct the packet with a new path
                    if let Some(fragments) = self.pending_messages.get(&session_id) {
                        if let Some(packet) = fragments.get(nack.fragment_index as usize) {
                            if let Some(target_server) = packet.routing_header.hops.last() {
                                if let Some(new_path) = self.compute_route_excluding(target_server) {

                                    // sending route to SC
                                    let _ = self
                                        .sim_controller_send
                                        .send(ServerEvent::Route(new_path.clone()));

                                    let mut new_packet = packet.clone();
                                    new_packet.routing_header.hops = new_path;
                                    new_packet.routing_header.hop_index = 1;

                                    if let Some(next_hop) = new_packet.routing_header.hops.get(1) {
                                        //info!("Client {}: Resending fragment {} via new path: {:?}",self.id, nack.fragment_index, new_packet.routing_header.hops);

                                        self.send_packet_and_notify(new_packet.clone(), *next_hop);

                                        // Reset the counter after rerouting
                                        // self.nack_counter.remove(&key);
                                        return;
                                    }
                                }
                            }
                        }
                    }
                    warn!("ContentServer {}: Unable to find alternative path", self.id);
                } else if *counter<4 {
                    // Standard resend
                    if let Some(fragments) = self.pending_messages.get(&session_id) {
                        if let Some(packet) = fragments.get(nack.fragment_index as usize) {

                            //info!("Client {}: Attempt {} for fragment {}", self.id, counter, nack.fragment_index);

                            self.send_packet_and_notify(packet.clone(), packet.routing_header.hops[1]);
                        }
                    }
                }
            }
            _ => {
                // Handling other NACK types
                self.network_discovery();
                if let Some(fragments) = self.pending_messages.get(&session_id) {
                    if let Some(packet) = fragments.get(nack.fragment_index as usize) {
                        self.send_packet_and_notify(packet.clone(), packet.routing_header.hops[1]);
                    }
                }
            }
        }
    }
    fn compute_route_excluding(&self, target_client: &NodeId) -> Option<Vec<NodeId>> {
        let mut visited = HashSet::new(); // Set to keep track of visited nodes during BFS.
        let mut queue = VecDeque::new(); // Queue for BFS traversal.
        let mut predecessors = HashMap::new(); // Map to store predecessors for path reconstruction.

        queue.push_back(self.id); // Start BFS from the client's own ID.
        visited.insert(self.id); // Mark client node as visited.

        while let Some(current_node) = queue.pop_front() { // While there are nodes in the queue.
            if current_node == *target_client { // If the current node is the target server.
                let mut path = Vec::new();
                let mut current = *target_client;
                while let Some(prev) = predecessors.get(&current) { // Reconstruct path by backtracking from the target server using predecessors.
                    path.push(current);
                    current = *prev;
                }
                path.push(self.id); // Add the client's ID to the path.
                path.reverse(); // Reverse the path to get the correct order from client to server.
                return Some(path); // Return the computed path.
            }

            // Iterate over neighbors excluding problematic nodes
            for &(a, b) in &self.topology { // Iterate through the network topology (edges).
                if a == current_node && !self.excluded_nodes.contains(&b) && !visited.contains(&b) { // If 'b' is a neighbor of 'a', 'b' is not excluded, and 'b' is not visited.
                    visited.insert(b); // Mark 'b' as visited.
                    queue.push_back(b); // Add 'b' to the queue for further exploration.
                    predecessors.insert(b, a); // Set 'a' as the predecessor of 'b'.
                } else if b == current_node && !self.excluded_nodes.contains(&a) && !visited.contains(&a) { // If 'a' is a neighbor of 'b', 'a' is not excluded and 'a' is not visited.
                    visited.insert(a); // Mark 'a' as visited.
                    queue.push_back(a); // Add 'a' to the queue.
                    predecessors.insert(a, b); // Set 'b' as the predecessor of 'a'.
                }
            }
        }

        let _send_sc = self.sim_controller_send.send(ServerEvent::Error(self.id, target_client.clone(), "not alternative path route available by server".to_string()));
        None // Return None if no path is found.
    }
}
