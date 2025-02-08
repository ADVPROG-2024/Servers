use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::time::Duration;

use crossbeam_channel::{select_biased, Receiver, Sender};
use dronegowski_utils::functions::{assembler, deserialize_message, fragment_message, generate_unique_id};
use dronegowski_utils::hosts::{
    ClientMessages, ServerCommand, ServerEvent, ServerMessages, ServerType, TestMessage,
};
use serde::de::DeserializeOwned;
use wg_2024::network::{NodeId, SourceRoutingHeader};
use wg_2024::packet::{FloodRequest, FloodResponse, Fragment, NodeType, Packet, PacketType};

use crate::DronegowskiServer;

#[derive(Clone)]
pub struct CommunicationServer {
    id: NodeId,
    sim_controller_send: Sender<ServerEvent>,
    sim_controller_recv: Receiver<ServerCommand>,
    packet_send: HashMap<NodeId, Sender<Packet>>,
    packet_recv: Receiver<Packet>,
    topology: HashSet<(NodeId, NodeId)>,
    node_types: HashMap<NodeId, NodeType>,
    message_storage: HashMap<u64, Vec<Fragment>>,
    server_type: ServerType, // Keep track of the server type
    registered_client: Vec<NodeId>,
}

impl DronegowskiServer for CommunicationServer {
    fn new(
        id: NodeId,
        sim_controller_send: Sender<ServerEvent>,
        sim_controller_recv: Receiver<ServerCommand>,
        packet_recv: Receiver<Packet>,
        packet_send: HashMap<NodeId, Sender<Packet>>,
        server_type: ServerType,
    ) -> Self {
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
            registered_client: Vec::new(),
        };

        server.network_discovery(); // Initial network discovery
        server
    }

    fn run(&mut self) {
        loop {
            select_biased! {
                recv(self.packet_recv) -> packet_res => {
                    if let Ok(packet) = packet_res {
                        self.handle_packet(packet);
                    }
                }
                recv(self.sim_controller_recv) -> command_res => { // Add command handling
                    if let Ok(command) = command_res {
                        self.handle_server_command(command);
                    }
                }
            }
        }
    }

    fn network_discovery(&self) {
        let flood_request = FloodRequest {
            flood_id: generate_unique_id(),
            initiator_id: self.id,
            path_trace: vec![(self.id, NodeType::Server)],
        };

        for (&node_id, sender) in &self.packet_send {
            let packet = Packet {
                pack_type: PacketType::FloodRequest(flood_request.clone()),
                routing_header: SourceRoutingHeader {
                    hop_index: 0,
                    hops: vec![self.id, node_id],
                },
                session_id: flood_request.flood_id,
            };
            let _ = sender.send(packet); // Handle potential send errors
        }
    }

    fn handle_packet(&mut self, packet: Packet) {
        let client_id = packet
            .routing_header
            .source()
            .expect("Packet must have a source");
        let key = packet.session_id;

        match packet.pack_type {
            PacketType::MsgFragment(fragment) => {
                self.message_storage
                    .entry(key)
                    .or_insert_with(Vec::new)
                    .push(fragment);

                if let Some(fragments) = self.message_storage.get(&key) {
                    if fragments.len() as u64 == fragments[0].total_n_fragments {
                        match self.reconstruct_message(key) {
                            Ok(message) => {
                                if let TestMessage::WebServerMessages(client_message) = message {
                                     self.handle_client_message(client_id, client_message);
                                }
                            }
                            Err(e) => log::error!("Error reconstructing message: {}", e),
                        }
                    }
                }
            }
            PacketType::FloodResponse(flood_response) => {
                self.update_graph(flood_response.path_trace);
            }
            PacketType::FloodRequest(flood_request) => {
                self.handle_flood_request(packet, flood_request);
            }
            PacketType::Ack(_) | PacketType::Nack(_) => {
                // TODO: Handle ACKs/NACKs (optional, for reliable communication)
            }
        }
    }

    fn handle_server_command(&mut self, command: ServerCommand) {
        match command {
            ServerCommand::SendFile(client_id, file) => {
                if let Some(path) = self.compute_best_path(client_id) {

                        self.send_message(
                            ServerMessages::File(file),
                            path,
                        );

                } else {
                    log::warn!("No route to client: {}", client_id);
                }
            }
        }
    }
    // New method to handle client messages.
    fn handle_client_message(&mut self, client_id: NodeId, client_message: ClientMessages) {

        match client_message {
            ClientMessages::ServerType => self.send_my_type(client_id),
            ClientMessages::RegistrationToChat => self.register_client(client_id),
            ClientMessages::ClientList | ClientMessages::MessageFor(_, _) => {
                if self.registered_client.contains(&client_id) {
                    match client_message {
                        ClientMessages::ClientList => self.send_register_client(client_id),
                        ClientMessages::MessageFor(target_id, message) => {
                            if self.registered_client.contains(&target_id) {
                                self.forward_message(target_id, client_id, message);
                            } else {
                                log::warn!("Target client {} not registered", target_id);
                            }
                        }
                        _ => {}
                    }
                } else {
                    log::warn!("Client {} not registered", client_id);
                }
            }
            _ => log::warn!("Unknown ClientMessage received"),
        }
    }


    fn send_message(&mut self, message: ServerMessages, route: Vec<NodeId>) {

        if let Some(serialized_data) = bincode::serialize(&message).ok() {
            let packets = fragment_message(&serialized_data, route.clone(), generate_unique_id());
            if let Some(next_hop) = route.get(1){
                if let Some(sender) = self.packet_send.get(next_hop) {
                   for packet in packets {
                        let _ = sender.send(packet);
                    }
                } else {
                    log::error!("No sender found for next hop: {}", next_hop);
                }
            } else{
                log::error!("Invalid route: {:?}", route);
            }

        } else {
             log::error!("Failed to serialize message");
        }

    }


    fn update_graph(&mut self, path_trace: Vec<(NodeId, NodeType)>) {
        log::info!("Updating graph with: {:?}", path_trace);
        for i in 0..path_trace.len() - 1 {
            let (node_a, _) = path_trace[i];
            let (node_b, _) = path_trace[i + 1];
            self.topology.insert((node_a, node_b));
            self.topology.insert((node_b, node_a)); // Ensure graph is bidirectional
        }

        for (node_id, node_type) in path_trace {
            self.node_types.insert(node_id, node_type);
        }
    }

    fn compute_best_path(&self, target_client: NodeId) -> Option<Vec<NodeId>> {
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

    fn reconstruct_message<T: DeserializeOwned>(&mut self, key: u64) -> Result<T, Box<dyn Error>> {
        if let Some(fragments) = self.message_storage.remove(&key) { // Remove on retrieval
            let mut sorted_fragments = fragments;
            sorted_fragments.sort_by_key(|f| f.fragment_index);

            let mut full_data = Vec::new();
            for fragment in sorted_fragments {
                assembler(&mut full_data, &fragment);
            }

            let message: T = bincode::deserialize(&full_data)?;
            Ok(message)
        } else {
            Err("No fragments found for key".into())
        }
    }
}

impl CommunicationServer {
    fn send_my_type(&mut self, client_id: NodeId) {
        if let Some(best_path) = self.compute_best_path(client_id) {
            self.send_message(ServerMessages::ServerType(self.server_type.clone()), best_path);
        } else { // added else
            log::warn!("No route to client: {}", client_id);
        }
    }


    fn register_client(&mut self, client_id: NodeId) {
        if let Some(path) = self.compute_best_path(client_id) {
            if self.registered_client.contains(&client_id) {
                self.send_message(
                    ServerMessages::RegistrationError("Client already registered".to_string()),
                    path,
                );
            } else {
                self.registered_client.push(client_id);
                self.send_message(ServerMessages::RegistrationOk, path);
            }
        } else { // added else
            log::warn!("No route to client: {}", client_id);
        }
    }

    fn send_register_client(&mut self, client_id: NodeId) {
        if let Some(path) = self.compute_best_path(client_id) {
            let data = ServerMessages::ClientList(self.registered_client.clone());
            self.send_message(data, path);
        } else { // added else
            log::warn!("No route to client: {}", client_id);
        }
    }

    fn forward_message(&mut self, target_id: NodeId, client_id: NodeId, message: String) {
        if let Some(path) = self.compute_best_path(target_id) {
            let final_message = ServerMessages::MessageFrom(client_id, message);
            self.send_message(final_message, path);
        } else { // added else
            log::warn!("No route to client: {}", target_id);
        }
    }

    fn handle_flood_request(&mut self, packet:Packet, flood_request: FloodRequest) {

        // 1. Update the graph *immediately*.
        self.update_graph(flood_request.path_trace.clone());

        // 2. Create a *new* path trace for the response.  This includes the server.
        let mut response_path_trace = flood_request.path_trace.clone();
        response_path_trace.push((self.id, NodeType::Server));

        // 3. Create the FloodResponse.
        let flood_response = FloodResponse {
            flood_id: flood_request.flood_id,
            path_trace: response_path_trace,  // Use the *new* path trace.
        };

        // 4. Create the response packet.  Reverse the *original* path for routing.
        let response_packet = Packet {
            pack_type: PacketType::FloodResponse(flood_response),
            routing_header: SourceRoutingHeader {
                hop_index: 0,
                hops: flood_request.path_trace.iter().rev().map(|(id, _)| *id).collect(),
            },
            session_id: packet.session_id,
        };

        // 5. Send the response back to the source.
        let source_id = packet.routing_header.source().expect("FloodRequest must have source");
         if let Some(sender) = self.packet_send.get(&source_id) {
                    match sender.send_timeout(response_packet.clone(), Duration::from_millis(500)) {
                        Err(_) => {
                            log::warn!("CommunicationServer {}: Timeout sending packet to {}", self.id, source_id);
                        }
                        Ok(..)=>{
                            log::info!("CommunicationServer {}: Sent FloodResponse back to {}", self.id, source_id);
                        }
                    }
                } else {
                    log::warn!("CommunicationServer {}: No sender found for node {}", self.id, source_id);
                }

        // The server does *not* forward FloodRequests.
    }
}
