use std::collections::{HashMap, HashSet, VecDeque};
use std::error::Error;
use std::process::Command;
use std::time::Duration;
use crossbeam_channel::{select_biased, unbounded, Receiver, Sender};
use dronegowski_utils::functions::{assembler, fragment_message, generate_unique_id};
use dronegowski_utils::hosts::{ClientMessages, ServerCommand, ServerEvent, ServerMessages, ServerType, TestMessage};
use dronegowski_utils::hosts::ServerType::Communication;
use log::{error, info, log, warn};
use serde::de::DeserializeOwned;
use wg_2024::network::{NodeId, SourceRoutingHeader};
use wg_2024::packet::{Ack, FloodRequest, FloodResponse, Fragment, Nack, NackType, NodeType, Packet, PacketType};
use crate::DronegowskiServer;

#[derive(Clone)]
pub struct CommunicationServer {
    id: NodeId,  // Unique identifier for the server

    // Channels for communication
    sim_controller_send: Sender<ServerEvent>,           // Channel to send commands to the Simulation Controller (SC)
    sim_controller_recv: Receiver<ServerCommand>,       // Channel to receive commands from the SC
    packet_send: HashMap<NodeId, Sender<Packet>>,       // Map of sending channels to neighbor nodes
    packet_recv: Receiver<Packet>,                      // Channel to receive packets from nodes

    // Network-related fields
    topology: HashSet<(NodeId, NodeId)>,                // Edges of the network graph
    node_types: HashMap<NodeId, NodeType>,              // Types of nodes (Client, Drone, Server)
    message_storage: HashMap<u64, Vec<Fragment>>,       // Storage for reassembling fragmented messages
    server_type: ServerType,                            // Type of the server (e.g., Communication)
    registered_client: Vec<NodeId>,                     // List of registered clients

    // Fields for handling acknowledgments (ACKs) and negative acknowledgments (NACKs)
    pending_messages: HashMap<u64, Vec<Packet>>,        // Storage for not yet acknowledged fragments
    acked_fragments: HashMap<u64, HashSet<u64>>,        // Storage for acknowledged fragments
    nack_counter: HashMap<(u64, u64, NodeId), u64>,      // Counter for NACKs per fragment, session, and node
    excluded_nodes: HashSet<NodeId>,                    // Nodes excluded from routing due to failures
}

impl DronegowskiServer for CommunicationServer {

    fn run(&mut self) {
        loop {
            // Use select_biased to prioritize packet reception over command reception
            select_biased! {
                recv(self.packet_recv) -> packet_res => {
                    // Handle received packet from neighbors
                    if let Ok(packet) = packet_res {
                        self.handle_packet(packet);
                    }
                },
                recv(self.sim_controller_recv) -> command_res => {
                    // Handle received command from the Simulation Controller
                    if let Ok(command) = command_res {
                        self.handle_command(command);
                    }
                }
            }
        }
    }

    fn network_discovery(&self) {
        // Start network discovery by creating a path trace
        let mut path_trace = Vec::new();
        path_trace.push((self.id, NodeType::Server));

        // Create a FloodRequest to discover the network
        let flood_request = FloodRequest {
            flood_id: generate_unique_id(),  // Generate a unique ID for the flood request
            initiator_id: self.id,          // ID of the initiating server
            path_trace,                      // Path trace to track the route
        };

        // Send the FloodRequest to all neighbor nodes
        for (node_id, sender) in &self.packet_send {
            // Create a packet for the FloodRequest
            let flood_request_packet = Packet {
                pack_type: PacketType::FloodRequest(flood_request.clone()),
                routing_header: SourceRoutingHeader {
                    hop_index: 0,
                    hops: vec![self.id, *node_id],
                },
                session_id: flood_request.flood_id,
            };

            // Send the FloodRequest packet to the neighbor
            let _send_neighbor = sender.send(flood_request_packet.clone());

            // Notify the Simulation Controller that a FloodRequest was sent
            let _send_sc = self
                .sim_controller_send
                .send(ServerEvent::PacketSent(flood_request_packet.clone()));
        }
    }

    fn handle_packet(&mut self, packet: Packet) {
        // Handle received packet
        if let Some(client_id) = packet.routing_header.source() {  // Get the source of the packet
            let key = packet.session_id;  // Identify the session ID
            match packet.pack_type {
                PacketType::MsgFragment(ref fragment) => {
                    // Handle received message fragment from a client
                    let _ = self
                        .sim_controller_send
                        .send(ServerEvent::DebugMessage(self.id, format!("Client {}: received from {}", self.id, client_id)));

                    // Send an ACK for the received fragment
                    self.send_ack(packet.clone(), fragment.clone());

                    // Add the fragment to the message storage for reassembly
                    self.message_storage
                        .entry(key)
                        .or_insert_with(Vec::new)
                        .push(fragment.clone());

                    // Check if all fragments have arrived
                    if let Some(fragments) = self.message_storage.get(&key) {
                        if let Some(first_fragment) = fragments.first() {
                            if fragments.len() as u64 == first_fragment.total_n_fragments {
                                // All fragments have arrived, start reassembling the message
                                match self.reconstruct_message(key) {
                                    Ok(message) => {
                                        // Message reassembled successfully
                                        if let TestMessage::WebServerMessages(client_message) = message {

                                            // Send the received message to the Simulation Controller
                                            let _send_sc = self
                                                .sim_controller_send
                                                .send(ServerEvent::MessageReceived(TestMessage::WebServerMessages(client_message.clone())));

                                            match client_message {  // Handle different types of client messages
                                                ClientMessages::ServerType => {
                                                    // Send the server type to the client
                                                    self.send_my_type(client_id)
                                                },
                                                ClientMessages::RegistrationToChat => {
                                                    // Register the client to the server
                                                    self.register_client(client_id)
                                                },
                                                ClientMessages::ClientList => {
                                                    // Send the list of registered clients to the client
                                                    self.send_register_client(client_id);
                                                },
                                                ClientMessages::MessageFor(target_id, message) => {
                                                    // Check if both source and target clients are registered
                                                    if self.registered_client.contains(&client_id) {
                                                        if self.registered_client.contains(&target_id) {
                                                            // Forward the message to the target client
                                                            self.forward_message(target_id, client_id, message)
                                                        } else {
                                                            // Target client is not registered
                                                            log::error!("Target client not registered");
                                                            // Send an error message to the client
                                                            self.send_message(ServerMessages::Error(format!("{} not registered to server", target_id)), client_id);
                                                        }
                                                    } else {
                                                        // Source client is not registered
                                                        log::error!("Client not registered");
                                                        // Send an error message to the client
                                                        self.send_message(ServerMessages::Error(format!("{} not registered to server", client_id)), client_id);
                                                    }

                                                },
                                                _ => {
                                                    // Handle unknown client messages
                                                    self.send_message(ServerMessages::Error(format!("Unknown ClientMessage received")), client_id);
                                                },
                                            }
                                        }
                                    }
                                    Err(e) => {  // Error occurred during message reassembly
                                        println!("Error reconstructing the message: {}", e);
                                    }
                                }
                            }
                        }
                    }
                }
                PacketType::FloodResponse(flood_response) => {
                    // Handle FloodResponse to update the network graph
                    info!("CommuncationServer {}: Received FloodResponse: {:?}", self.id, flood_response);
                    self.update_graph(flood_response.path_trace);
                }
                PacketType::FloodRequest(ref flood_request) => {
                    // Update the graph with the path trace from the FloodRequest
                    // self.update_graph(flood_request.path_trace.clone());

                    info!("CommuncationServer {}: Received FloodRequest: {:?}", self.id, flood_request);

                    // Create a new path trace for the FloodResponse, including this server
                    let mut response_path_trace = flood_request.path_trace.clone();
                    response_path_trace.push((self.id, NodeType::Server));

                    // Create the FloodResponse
                    let flood_response = FloodResponse {
                        flood_id: flood_request.flood_id,
                        path_trace: response_path_trace.clone(),  // Use the new path trace
                    };

                    // Create the response packet with reversed routing path
                    let response_packet = Packet {
                        pack_type: PacketType::FloodResponse(flood_response),
                        routing_header: SourceRoutingHeader {
                            hop_index: 1,
                            hops: response_path_trace.iter().rev().map(|(id, _)| *id).collect(),
                        },
                        session_id: packet.session_id,
                    };

                    // Send the FloodResponse back to the source
                    info!("CommuncationServer {}: Sending FloodResponse: {:?}", self.id, response_packet);
                    let next_node = response_packet.routing_header.hops[1];
                    self.send_packet_and_notify(response_packet, next_node);

                }

                PacketType::Ack(ack) => {
                    // Handle received ACK from a source
                    self.handle_ack(ack.clone(), packet.session_id);
                }
                PacketType::Nack(ref nack) => {
                    // Handle received NACK from a source
                    let drop_drone = packet.clone().routing_header.hops[0];
                    self.handle_nack(nack.clone(), packet.session_id, drop_drone);
                }
                _ => {
                    // Handle unhandled packet types
                    log::error!("CommunicationServer {}: Received unhandled packet type", self.id);
                }
            }
        }
    }

    fn handle_command(&mut self, command: ServerCommand) {
        match command {
            ServerCommand::AddSender(id, sender) => {
                // Add a new neighbor to the server's sender map
                self.add_neighbor(id, sender);
                self.network_discovery();
            },
            ServerCommand::RemoveSender(id) => {
                // Remove a neighbor from the server's sender map
                self.remove_neighbor(id);
                self.network_discovery();
            },
            ServerCommand::ControllerShortcut(packet) => {
                // Handle a packet received directly from the Simulation Controller
                self.handle_packet(packet);
            },
            ServerCommand::RequestNetworkDiscovery => {self.network_discovery()},
            _ =>{
                // Handle unhandled command types
                log::error!("CommunicationServer {}: Received unhandled ServerCommand type", self.id);
            }
        }
    }

    fn update_graph(&mut self, path_trace: Vec<(NodeId, NodeType)>) {
        // Update the network graph with the new path trace
        for i in 0..path_trace.len() - 1 {
            let (node_a, _node_type) = path_trace[i];  // Get the first node in the edge
            let (node_b, _node_type) = path_trace[i + 1];  // Get the second node in the edge
            self.topology.insert((node_a, node_b));  // Add the edge to the topology
            self.topology.insert((node_b, node_a));  // Ensure the graph is bidirectional
        }

        // Update the node types in the graph
        for (node_id, node_type) in path_trace {
            self.node_types.insert(node_id, node_type);
        }
    }

    fn compute_best_path(&self, target_client: NodeId) -> Option<Vec<NodeId>> {
        use std::collections::VecDeque;

        // Use BFS to compute the best path to the target client
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        let mut predecessors = HashMap::new();

        queue.push_back(self.id);
        visited.insert(self.id);

        while let Some(current) = queue.pop_front() {
            if current == target_client {
                // Reconstruct the path from the target to the source
                let mut path = vec![current];
                while let Some(&pred) = predecessors.get(&path[0]) {
                    path.insert(0, pred);
                }
                return Some(path);
            }

            // Explore neighbors
            for &(node_a, node_b) in &self.topology {
                if node_a == current && !visited.contains(&node_b) {
                    if let Some(node_type) = self.node_types.get(&node_b) {
                        if *node_type != NodeType::Client || node_b == target_client {
                            visited.insert(node_b);
                            queue.push_back(node_b);
                            predecessors.insert(node_b, current);
                        }
                    }
                }
            }
        }
        None  // Return None if no path is found
    }

    fn reconstruct_message<T: DeserializeOwned>(&mut self, key: u64) -> Result<T, Box<dyn Error>> {
        // Reassemble a message from its fragments
        if let Some(fragments) = self.message_storage.clone().get(&key) {
            if let Some(first_fragment) = fragments.first() {
                if fragments.len() as u64 == first_fragment.total_n_fragments {
                    self.message_storage.remove(&key);
                    // Sort fragments by their index
                    let mut fragment_map: HashMap<u64, &Fragment> = HashMap::new();
                    for fragment in fragments {
                        fragment_map.insert(fragment.fragment_index, fragment);
                    }

                    // Reassemble the full message data
                    let mut full_data = Vec::new();
                    for index in 0..fragments.len() as u64 {
                        if let Some(fragment) = fragment_map.get(&index) {
                            assembler(&mut full_data, fragment);
                        } else {
                            return Err(format!("Missing fragment with index: {}", index).into());
                        }
                    }

                    // Deserialize the full message
                    let message: T = bincode::deserialize(&full_data)?;
                    Ok(message)
                } else {
                    Err(format!(
                        "Total number of fragments ({}) does not match the vector length ({})",
                        first_fragment.total_n_fragments,
                        fragments.len()
                    )
                        .into())
                }
            } else {
                Err("No fragments found in the list".into())
            }
        } else {
            Err(format!("No fragments found for the key: {}", key).into())
        }
    }

    fn add_neighbor(&mut self, node_id: NodeId, sender: Sender<Packet>) {
        // Add a new neighbor to the server's sender map
        if let std::collections::hash_map::Entry::Vacant(e) = self.packet_send.entry(node_id) {
            e.insert(sender);
        } else {
            log::error!("CommunicationServer {}: Sender for node {node_id} already stored in the map!", self.id);
        }
    }

    fn remove_neighbor(&mut self, node_id: NodeId) {
        // Remove a neighbor from the server's sender map
        if self.packet_send.contains_key(&node_id) {
            self.packet_send.remove(&node_id);
            self.remove_from_topology(node_id);  // Remove the node from the topology
        } else {
            log::error!("CommunicationServer {}: the {} is not a neighbour", self.id, node_id);
        }
    }

    fn remove_from_topology(&mut self, node_id: NodeId) {
        // Remove a node from the network topology
        self.topology.retain(|&(a, b)| a != node_id && b != node_id);
        self.node_types.remove(&node_id);
    }
}

impl CommunicationServer {
    pub fn new(id: NodeId, sim_controller_send: Sender<ServerEvent>, sim_controller_recv: Receiver<ServerCommand>, packet_recv: Receiver<Packet>, packet_send: HashMap<NodeId, Sender<Packet>>, server_type: ServerType) -> Self {

        // Initialize a new CommunicationServer instance
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
            pending_messages: HashMap::new(),
            acked_fragments: HashMap::new(),
            nack_counter: HashMap::new(),
            excluded_nodes: HashSet::new(),
        };

        server.network_discovery();  // Trigger network discovery after initialization

        server
    }

    fn send_my_type(&mut self, client_id: NodeId) {
        // Send the server type to the client
        self.send_message(ServerMessages::ServerType(self.clone().server_type), client_id);
    }

    fn register_client(&mut self, client_id: NodeId) {
        // Register a client to the server
        if self.registered_client.contains(&client_id) {
            log::error!("Communication server {}: client {} already registered", self.id, client_id);
            self.send_message(ServerMessages::RegistrationError("client already registered".to_string()), client_id);
        } else {
            self.registered_client.push(client_id.clone());
            self.send_message(ServerMessages::RegistrationOk, client_id);
        }
    }

    fn send_register_client(&mut self, client_id: NodeId) {
        // Send the list of registered clients to the client
        let data = ServerMessages::ClientList(self.clone().registered_client);
        self.send_message(data, client_id)
    }

    fn forward_message(&mut self, target_id: NodeId, client_id: NodeId, message: String) {
        // Forward a message from one client to another
        let final_message = ServerMessages::MessageFrom(client_id, message);
        self.send_message(final_message, target_id);
    }

    fn send_message(&mut self, message: ServerMessages, destination: NodeId) {
        // Attempt to compute the best path to the destination.
        // If no valid path is found, an empty vector is used as a fallback.
        let route = self.compute_best_path(destination).unwrap_or(Vec::new());

        // sending route to SC
        let _send_sc = self
            .sim_controller_send
            .send(ServerEvent::Route(route.clone()));

        // Check if the computed route has at least two nodes (source and next-hop).
        if let Some(&neighbour_id) = route.get(1) {
            // The next-hop neighbor ID is the second element in the computed route.

            // Try to retrieve the sending channel (packet_send) associated with the neighbor.
            if let Some(sender) = self.packet_send.get(&neighbour_id) {
                // Generate a unique session ID for this message.
                let session_id = generate_unique_id();

                // Fragment the message into smaller packets for transmission.
                // It wraps the `ServerMessages` inside a `TestMessage::WebClientMessages` variant.
                let packets = fragment_message(&TestMessage::WebClientMessages(message), route, session_id);

                // Store the generated packets in the `pending_messages` map using the session ID as the key.
                self.pending_messages.insert(session_id, packets.clone());

                // Iterate over each packet and send it to the next hop.
                for mut packet in packets {
                    // Set the hop index to 1, meaning the packet is at the first hop in the route.
                    packet.routing_header.hop_index = 1;

                    // Send the packet using the retrieved sender channel.
                    sender.send(packet.clone()).expect("Error occurred sending the message to the neighbour.");

                    // Notify the simulation controller (sim_controller_send) that a packet was sent.
                    let _ = self
                        .sim_controller_send
                        .send(ServerEvent::PacketSent(packet.clone()));
                }
            } else {
                // If no sending channel is found for the next-hop neighbor, log an error.
                log::error!("ContentServer {}: Neighbour {} not found!", self.id, neighbour_id);
            }
        } else {
            // If no valid route is found, log an error.
            log::error!("ContentServer {}: There is no available route", self.id);
        }
    }

    fn send_ack(&mut self, packet: Packet, fragment: Fragment) {
        // Send an ACK for a received fragment
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
            if let Some(sender) = self.packet_send.get(&next_hop) {
                sender.send(ack_packet.clone()).expect("Error occurred sending the ack to the neighbour.");
                // Notify the Simulation Controller of packet sending
                let _ = self
                    .sim_controller_send
                    .send(ServerEvent::PacketSent(ack_packet.clone()));
            } else {
                log::error!("CommunicationServer {}: Neighbour {} not found!", self.id, next_hop);
            }
        } else {
            log::warn!("CommunicationServer {}: No valid path to send Ack for fragment {}", self.id, fragment.fragment_index);
        }
    }

    fn handle_ack(&mut self, ack: Ack, session_id: u64) {
        // Handle received ACK
        let fragment_index = ack.fragment_index;

        // Remove the fragment from the NACK counter
        self.nack_counter.retain(|(f_idx, s_id, _), _| !(*f_idx == fragment_index && *s_id == session_id));

        // Update the set of acknowledged fragments
        let acked = self.acked_fragments.entry(session_id).or_default();
        acked.insert(fragment_index);

        // Check if all fragments of the session have been acknowledged
        if let Some(fragments) = self.pending_messages.get(&session_id) {
            let total_fragments = fragments.len() as u64;
            if acked.len() as u64 == total_fragments {
                // Remove the session from pending and acknowledged storage
                self.pending_messages.remove(&session_id);
                self.acked_fragments.remove(&session_id);
            }
        }
    }

    fn handle_nack(&mut self, nack: Nack, session_id: u64, id_drop_drone: NodeId) {
        // Handle received NACK
        let key = (nack.fragment_index, session_id, id_drop_drone);

        // Increment the NACK counter for the fragment
        let counter = self.nack_counter.entry(key).or_insert(0);
        *counter += 1;

        match nack.nack_type {
            NackType::Dropped => {

                if *counter == 4 {
                    // Too many NACKs, calculate an alternative path

                    let _ = self
                        .sim_controller_send
                        .send(ServerEvent::DebugMessage(self.id, format!("Server {}: nack drop {} from {} / {}", self.id, counter, id_drop_drone, nack.fragment_index)));

                    info!("Client {}: Too many NACKs for fragment {}. Calculating alternative path", self.id, nack.fragment_index);

                    // Exclude the problematic node from future routing
                    self.excluded_nodes.insert(id_drop_drone);

                    let _ = self
                        .sim_controller_send
                        .send(ServerEvent::DebugMessage(self.id, format!("Server {}: new route exclude {:?}", self.id, self.excluded_nodes)));

                    // Reconstruct the packet with a new path
                    if let Some(fragments) = self.pending_messages.get(&session_id) {
                        if let Some(packet) = fragments.get(nack.fragment_index as usize) {
                            if let Some(target_server) = packet.routing_header.hops.last() {
                                if let Some(new_path) = self.compute_route_excluding(target_server) {

                                    // sending route to SC
                                    let _send_sc = self
                                        .sim_controller_send
                                        .send(ServerEvent::Route(new_path.clone()));

                                    let mut new_packet = packet.clone();
                                    new_packet.routing_header.hops = new_path;
                                    new_packet.routing_header.hop_index = 1;

                                    if let Some(next_hop) = new_packet.routing_header.hops.get(1) {
                                        info!("Client {}: Resending fragment {} via new path: {:?}",
                                        self.id, nack.fragment_index, new_packet.routing_header.hops);
                                        self.send_packet_and_notify(new_packet.clone(), *next_hop);  // Resend the packet

                                        // Reset the NACK counter after rerouting
                                        // self.nack_counter.remove(&key);
                                        return;
                                    }
                                }
                            }
                        }
                    }
                    warn!("CommunicationServer {}: Unable to find alternative path", self.id);
                } else if *counter<4 {
                    // Standard resend of the fragment
                    if let Some(fragments) = self.pending_messages.get(&session_id) {
                        if let Some(packet) = fragments.get(nack.fragment_index as usize) {
                            info!("Communication server {}: Attempt {} for fragment {}",
                            self.id, counter, nack.fragment_index);
                            self.send_packet_and_notify(packet.clone(), packet.routing_header.hops[1]);
                        }
                    }
                }
            }
            _ => {
                // Handle other types of NACKs (e.g., network discovery)
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
                    if a == current_node && !visited.contains(&b) {
                        if let Some(node_type) = self.node_types.get(&b) {
                            if *node_type != NodeType::Client || b == *target_client {
                                visited.insert(b); // Mark 'b' as visited.
                                queue.push_back(b); // Add 'b' to the queue for further exploration.
                                predecessors.insert(b, a); // Set 'a' as the predecessor of 'b'.
                            }
                        }
                    }
                } else if b == current_node && !self.excluded_nodes.contains(&a) && !visited.contains(&a) { // If 'a' is a neighbor of 'b', 'a' is not excluded and 'a' is not visited.
                    if let Some(node_type) = self.node_types.get(&a) {
                        if *node_type != NodeType::Client || a == *target_client {
                            visited.insert(a); // Mark 'a' as visited.
                            queue.push_back(a); // Add 'a' to the queue.
                            predecessors.insert(a, b); // Set 'b' as the predecessor of 'a'.
                        }
                    }
                }
            }
        }

        let _send_sc = self.sim_controller_send.send(ServerEvent::Error(self.id, target_client.clone(), "not alternative path route available by server".to_string()));
        None // Return None if no path is found.
    }

    fn send_packet_and_notify(&self, packet: Packet, recipient_id: NodeId) {
        // Send a packet to a recipient and notify the Simulation Controller
        if let Some(sender) = self.packet_send.get(&recipient_id) {
            if let Err(e) = sender.send(packet.clone()) {
                error!(
                    "CommunicationServer {}: Error sending packet to {}: {:?}",
                    self.id,
                    recipient_id,
                    e
                );
            } else {
                info!(
                    "CommunicationServer {}: Packet sent to {}: must arrive at {}",
                    self.id,
                    recipient_id,
                    packet.routing_header.hops.last().unwrap(),
                );

                // Notify the Simulation Controller of packet sending
                let _ = self
                    .sim_controller_send
                    .send(ServerEvent::PacketSent(packet));
            }
        } else {
            error!("CommunicationServer {}: No sender for node {}", self.id, recipient_id);
        }
    }
}