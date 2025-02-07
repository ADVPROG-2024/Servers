// use std::collections::{HashMap, HashSet};
// use std::time::Duration;
// use crossbeam_channel::{select_biased, unbounded, Receiver, Sender};
// use dronegowski_utils::functions::{assembler, fragment_message, generate_unique_id};
// use dronegowski_utils::hosts::{ClientMessages, ServerCommand, ServerEvent, ServerMessages, ServerType, TestMessage};
// use wg_2024::network::{NodeId, SourceRoutingHeader};
// use wg_2024::packet::{FloodRequest, FloodResponse, Fragment, NodeType, Packet, PacketType};
// use crate::DronegowskiServer;
// use serde::{Deserialize, Serialize};
// use serde::de::DeserializeOwned;
//
// #[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
// pub struct TextServer {
//     pub stored_texts: HashMap<u64, String>, // ID → Testo
// }
// impl TextServer {
//     pub fn new() -> TextServer {
//         Self{
//             stored_texts: HashMap::new(),
//         }
//     }
// }
// impl Default for TextServer {
//     fn default() -> Self {
//         Self::new()
//     }
// }
//
//
//
// #[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
// pub struct MediaServer {
//     pub stored_media: HashMap<u64, Vec<u8>>, // ID → Media
// }
// impl MediaServer {
//     pub fn new() -> MediaServer {
//         Self{
//             stored_media: HashMap::new(),
//         }
//     }
// }
// impl Default for MediaServer {
//     fn default() -> Self {
//         Self::new()
//     }
// }
//
//
//
// pub struct ContentServer {
//     id: NodeId,
//     sim_controller_send: Sender<ServerEvent>,           //Channel used to send commands to the SC
//     sim_controller_recv: Receiver<ServerCommand>,       //Channel used to receive commands from the SC
//     packet_send: HashMap<NodeId, Sender<Packet>>,       //Map containing the sending channels of neighbour nodes
//     packet_recv: Receiver<Packet>,                      //Channel used to receive packets from nodes
//     server_type: ServerType,
//     topology: HashSet<(NodeId, NodeId)>,                // Edges of the graph
//     node_types: HashMap<NodeId, NodeType>,              // Node types (Client, Drone, Server)
//     message_storage: HashMap<u64, Vec<Fragment>>,       // Store for reassembling messages
//     text: TextServer,
//     media: MediaServer,
// }
//
// impl DronegowskiServer for ContentServer {
//     fn new(id: NodeId, sim_controller_send: Sender<ServerEvent>, sim_controller_recv: Receiver<ServerCommand>, packet_recv: Receiver<Packet>, packet_send: HashMap<NodeId, Sender<Packet>>, server_type: ServerType) -> Self {
//
//         let mut server = Self {
//             id,
//             sim_controller_send,
//             sim_controller_recv,
//             packet_recv,
//             packet_send,
//             server_type,
//             message_storage: HashMap::new(),
//             topology: HashSet::new(),
//             node_types: HashMap::new(),
//             text: TextServer::default(),
//             media: MediaServer::new(),
//
//         };
//
//         server.network_discovery();
//
//         server
//     }
//
//     fn network_discovery(&self) {
//         let mut path_trace = Vec::new();
//         path_trace.push((self.id, NodeType::Server));
//
//         // Send flood_request to the neighbour nodes
//         let flood_request = FloodRequest {
//             flood_id: generate_unique_id(),
//             initiator_id: self.id,
//             path_trace,
//         };
//
//         for (node_id, sender) in &self.packet_send {
//             let _ = sender.send(Packet {
//                 pack_type: PacketType::FloodRequest(flood_request.clone()),
//                 routing_header: SourceRoutingHeader {
//                     hop_index: 0,
//                     hops: vec![self.id, *node_id],
//                 },
//                 session_id: flood_request.flood_id,
//             });
//         }
//     }
//
//     fn run(&mut self) {
//         loop {
//             select_biased! {
//                 recv(self.packet_recv) -> packet_res => {
//                     if let Ok(packet) = packet_res {
//                         self.handle_packet(packet);
//                     }
//                 },
//                 recv(self.sim_controller_recv) -> command_res => {
//                     if let Ok(command) = command_res {
//                         self.handle_command(command);
//                     }
//                 },
//             }
//         }
//     }
//
//     fn handle_packet(&mut self, packet: Packet){
//         if let Some(source_id) = packet.routing_header.source(){
//             let key = packet.session_id;
//             match packet.pack_type {
//                 PacketType::MsgFragment(fragment) => {
//                     self.message_storage
//                         .entry(key)
//                         .or_insert_with(Vec::new)
//                         .push(fragment);
//
//                     // Check if all the fragments are received
//                     if let Some(fragments) = self.message_storage.get(&key) {
//                         if let Some(first_fragment) = fragments.first() {
//                             if fragments.len() as u64 == first_fragment.total_n_fragments {
//                                 match self.reconstruct_message(key) {
//                                     Ok(message) => {
//                                         if let TestMessage::WebServerMessages(client_messages) = message {
//                                             match client_messages {
//                                                 ClientMessages::ServerType =>{
//                                                     self.send_message(ServerMessages::ServerType(ServerType::Content), source_id);
//                                                 },
//                                                 ClientMessages::FilesList =>{
//                                                     let list = self.list_files();
//                                                     self.send_message(ServerMessages::FilesList(list), source_id);
//                                                 },
//                                                 ClientMessages::File(file_id) =>{
//                                                     match self.get_file_text(file_id) {
//                                                         Some(text) => {self.send_message(ServerMessages::File(text), source_id);},
//                                                         None => {self.send_message(ServerMessages::Error("file not found".to_string()),source_id)},
//                                                     }
//                                                 },
//                                                 ClientMessages::Media(media_id) =>{
//                                                     match self.get_media(media_id) {
//                                                         Some(media) => self.send_message(ServerMessages::Media(media), source_id),
//                                                         None => {self.send_message(ServerMessages::Error("media not found".to_string()),source_id)},
//                                                     }
//                                                 },
//                                                 _ => {println!("Unkown message type");},
//                                             }
//                                         }
//                                     }
//                                     Err(e) => {
//                                         println!("Error reconstructing the message: {}", e);
//                                     }
//                                 }
//                             }
//                         }
//                     }
//                 }
//                 PacketType::FloodResponse(flood_response) => {
//                     // update the graph knowledge based on the new FloodResponse
//                     self.update_graph(flood_response.path_trace);
//                 }
//                 PacketType::FloodRequest(mut flood_request) => {
//                     self.send_flood_response(flood_request, packet.clone());
//                 }
//                 PacketType::Ack(ack) => {
//                     //gestire ack
//                 }
//                 PacketType::Nack(nack) => {
//                     //gestire nack
//                 }
//             }
//         }
//     }
//
//     fn send_message(&mut self, message: ServerMessages, destination: NodeId) {
//         let route=self.compute_best_path(destination).unwrap_or(Vec::new());
//
//         if let Some(&neighbour_id) = route.first() {
//             if let Some(sender) = self.packet_send.get(&neighbour_id) {
//                 let serialized_data = bincode::serialize(&message).expect("Serialization failed");
//                 let packets = fragment_message(&serialized_data, route, 1);
//
//                 for mut packet in packets {
//                     packet.routing_header.hop_index = 1;
//                     sender.send(packet).expect("Error occurred sending the message to the neighbour.");
//                 }
//             } else {
//                 println!("Error: Neighbour {} not found!", neighbour_id);
//             }
//         } else {
//             println!("Error: There is no available route");
//         }
//     }
//
//
//     fn update_graph(&mut self, path_trace: Vec<(NodeId, NodeType)>) {
//         log::info!("Aggiornamento del grafo con i dati ricevuti: {:?}", path_trace);
//         for i in 0..path_trace.len() - 1 {
//             let (node_a, _) = path_trace[i];
//             let (node_b, _) = path_trace[i + 1];
//             self.topology.insert((node_a, node_b));
//             self.topology.insert((node_b, node_a)); // Grafo bidirezionale
//         }
//
//         for (node_id, node_type) in path_trace {
//             self.node_types.insert(node_id, node_type);
//         }
//     }
//
//     fn compute_best_path(&self, target_client: NodeId) -> Option<Vec<NodeId>> {
//         use std::collections::VecDeque;
//
//         let mut visited = HashSet::new();
//         let mut queue = VecDeque::new();
//         let mut predecessors = HashMap::new();
//
//         queue.push_back(self.id);
//         visited.insert(self.id);
//
//         while let Some(current) = queue.pop_front() {
//             if current == target_client {
//                 let mut path = vec![current];
//                 while let Some(&pred) = predecessors.get(&path[0]) {
//                     path.insert(0, pred);
//                 }
//                 return Some(path);
//             }
//
//             for &(node_a, node_b) in &self.topology {
//                 if node_a == current && !visited.contains(&node_b) {
//                     visited.insert(node_b);
//                     queue.push_back(node_b);
//                     predecessors.insert(node_b, current);
//                 }
//             }
//         }
//
//         None
//     }
//
//     fn reconstruct_message<T: DeserializeOwned>(&mut self, key: u64) -> Result<T, Box<dyn std::error::Error>> {
//         // Identifica il vettore di frammenti associato alla chiave
//         if let Some(fragments) = self.message_storage.get(&key) {
//             if let Some(first_fragment) = fragments.first() {
//                 if fragments.len() as u64 == first_fragment.total_n_fragments {
//                     self.message_storage.remove(&key);
//                     // Crea una mappa indicizzata per ordinare i frammenti
//                     let mut fragment_map: HashMap<u64, &Fragment> = HashMap::new();
//                     for fragment in fragments {
//                         fragment_map.insert(fragment.fragment_index, fragment);
//                     }
//
//                     // Inizializza il buffer per i dati completi
//                     let mut full_data = Vec::new();
//
//                     // Usa l'assembler per ciascun frammento in ordine
//                     for index in 0..fragments.len() as u64 {
//                         if let Some(fragment) = fragment_map.get(&index) {
//                             assembler(&mut full_data, fragment);
//                         } else {
//                             return Err(format!("Frammento mancante con indice: {}", index).into());
//                         }
//                     }
//
//                     // Deserializza il messaggio completo
//                     let message: T = bincode::deserialize(&full_data)?;
//                     Ok(message)
//                 } else {
//                     Err(format!(
//                         "Il numero totale di frammenti ({}) non corrisponde alla lunghezza del vettore ({})",
//                         first_fragment.total_n_fragments,
//                         fragments.len()
//                     )
//                         .into())
//                 }
//             } else {
//                 Err("Nessun frammento trovato nella lista".into())
//             }
//         } else {
//             Err(format!("Nessun frammento trovato per la chiave: {}", key).into())
//         }
//     }
//
// }
//
// impl ContentServer {
//
//     fn send_flood_response(&mut self, mut flood_request: FloodRequest, packet: Packet) {
//         flood_request.path_trace.push((self.id, NodeType::Server));
//
//         let flood_response = FloodResponse {
//             flood_id: flood_request.flood_id,
//             path_trace: flood_request.path_trace.clone(),
//         };
//
//         let source_id = packet.routing_header.source().expect("FloodRequest must have a source");
//
//         let response_packet = Packet {
//             pack_type: PacketType::FloodResponse(flood_response),
//             routing_header: SourceRoutingHeader {
//                 hop_index: 0, // Reset hop_index
//                 hops: flood_request.path_trace.iter().rev().map(|(id, _)| *id).collect(),
//             },
//             session_id: packet.session_id,
//         };
//
//         if let Some(sender) = self.packet_send.get(&source_id) {
//             match sender.send_timeout(response_packet.clone(), Duration::from_millis(500)) {
//                 Err(_) => {
//                     log::warn!("ContentServer {}: Timeout sending packet to {}", self.id, source_id);
//                 }
//                 Ok(..)=>{
//                     log::info!("ContentServer {}: Sent FloodResponse back to {}", self.id, source_id);
//                 }
//             }
//         } else {
//             log::warn!("ContentServer {}: No sender found for node {}", self.id, source_id);
//         }
//     }
//
//     fn handle_command(&mut self, command: ServerCommand) {
//         log::info!("ContentServer {}: Received ServerCommand: {:?}", self.id, command);
//
//         match command {
//             ServerCommand::AddSender(id, sender) => {
//                 self.add_neighbor(id, sender);
//             }
//             ServerCommand::RemoveSender(id) => {
//                 self.remove_neighbor(id);
//             }
//             _ =>{
//                 // Unclassified Command
//             }
//         }
//     }
//     fn add_neighbor(&mut self, node_id: NodeId, sender: Sender<Packet>) {
//         if let std::collections::hash_map::Entry::Vacant(e) = self.packet_send.entry(node_id) {
//             e.insert(sender);
//             // send a FloodRequest
//         } else {
//             panic!("Sender for node {node_id} already stored in the map!");
//         }
//     }
//     fn remove_neighbor(&mut self, node_id: NodeId) {
//         if self.packet_send.contains_key(&node_id) {
//             self.packet_send.remove(&node_id);
//             // send a FloodRequest?
//         } else {
//             panic!("the {} is not neighbour of the drone {}", node_id, self.id);
//         }
//     }
//
//
//     fn list_files(&self) -> Vec<(u64, String)> {
//         self.text.stored_texts.iter()
//             .map(|(&id, content)| (id, content.clone()))
//             .collect()
//     }
//     fn get_file_text(&self, file_id: u64) -> Option<String> {
//         self.text.stored_texts.get(&file_id).cloned()
//     }
//     fn get_media(&self, media_id: u64) -> Option<Vec<u8>> {
//         self.media.stored_media.get(&media_id).cloned()
//     }
// }