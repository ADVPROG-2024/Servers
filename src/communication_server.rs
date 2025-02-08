use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::time::Duration;
use crossbeam_channel::{select_biased, unbounded, Receiver, Sender};
use dronegowski_utils::functions::{assembler, fragment_message, generate_unique_id};
use dronegowski_utils::hosts::{ClientMessages, ServerCommand, ServerEvent, ServerMessages, ServerType, TestMessage};
use serde::de::DeserializeOwned;
use wg_2024::network::{NodeId, SourceRoutingHeader};
use wg_2024::packet::{FloodRequest, FloodResponse, Fragment, NodeType, Packet, PacketType};
use crate::DronegowskiServer;

#[derive(Clone)]
pub struct CommunicationServer {
    id: NodeId,
    sim_controller_send: Sender<ServerEvent>,           //Channel used to send commands to the SC
    sim_controller_recv: Receiver<ServerCommand>,       //Channel used to receive commands from the SC
    packet_send: HashMap<NodeId, Sender<Packet>>,       //Map containing the sending channels of neighbour nodes
    packet_recv: Receiver<Packet>,                      //Channel used to receive packets from nodes
    topology: HashSet<(NodeId, NodeId)>,                // Edges of the graph
    node_types: HashMap<NodeId, NodeType>,              // Node types (Client, Drone, Server)
    message_storage: HashMap<u64, Vec<Fragment>>,       // Store for reassembling messages
    server_type: ServerType,
    registered_client: Vec<NodeId>,                     //typology of the server
}

impl DronegowskiServer for CommunicationServer {
    fn new(id: NodeId, sim_controller_send: Sender<ServerEvent>, sim_controller_recv: Receiver<ServerCommand>, packet_recv: Receiver<Packet>, packet_send: HashMap<NodeId, Sender<Packet>>, server_type: ServerType) -> Self {

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

        server.network_discovery();

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
            }
        }
    }

    fn network_discovery(&self) {
        let mut path_trace = Vec::new();
        path_trace.push((self.id, NodeType::Server));

        // Send flood_request to the neighbour nodes
        let flood_request = FloodRequest {
            flood_id: generate_unique_id(),
            initiator_id: self.id,
            path_trace,
        };

        for (node_id, sender) in &self.packet_send {
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

    fn handle_packet(&mut self, packet: Packet) {
        log::info!("Server {}: Received packet: {:?}", self.id, packet); // Log the received packet

        let client_id = packet.routing_header.source().unwrap(); // Identifica il client ID
        let key = packet.session_id; // Identifica la sessione
        match packet.pack_type {
            PacketType::MsgFragment(fragment) => {
                log::info!("Server {}: Received MsgFragment from {}, session: {}, index: {}, total: {}",self.id, client_id, key, fragment.fragment_index, fragment.total_n_fragments);

                // Aggiunta del frammento al message_storage
                self.message_storage
                    .entry(key)
                    .or_insert_with(Vec::new)
                    .push(fragment);

                // Verifica se tutti i frammenti sono stati ricevuti
                if let Some(fragments) = self.message_storage.get(&key) {
                    if let Some(first_fragment) = fragments.first() {
                        if fragments.len() as u64 == first_fragment.total_n_fragments {
                            // Tutti i frammenti sono stati ricevuti, tenta di ricostruire il messaggio
                            match self.reconstruct_message(key) {
                                Ok(message) => {
                                    log::info!("Server {}: Message reassembled successfully.", self.id);
                                    if let TestMessage::WebServerMessages(client_message) = message {
                                        match client_message {
                                            ClientMessages::ServerType => self.send_my_type(client_id),
                                            ClientMessages::RegistrationToChat => self.register_client(client_id),
                                            ClientMessages::ClientList | ClientMessages::MessageFor(_, _) => {
                                                if self.registered_client.contains(&client_id) {
                                                    match client_message {
                                                        ClientMessages::ClientList => self.send_register_client(client_id),
                                                        ClientMessages::MessageFor(target_id, message) => {
                                                            if self.registered_client.contains(&target_id) {
                                                                self.forward_message(target_id, client_id, message)
                                                            } else {
                                                                println!("target client not registered");
                                                            }
                                                        }
                                                        _ => {}
                                                    }
                                                } else {
                                                    println!("client not registered");
                                                }
                                            }
                                            _ => println!("Unknown ClientMessage received"),
                                        }
                                    }
                                }
                                Err(e) => {
                                    println!("Error reconstructing the message: {}", e);
                                }
                            }
                        }
                    }
                }
            }
            PacketType::FloodResponse(flood_response) => {
                // Gestisce la risposta di flooding aggiornando il grafo
                self.update_graph(flood_response.path_trace);
            }
            PacketType::FloodRequest(flood_request) => { // Non più mut flood_request
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

            }

            PacketType::Ack(ack) => {
                //gestire ack
            }
            PacketType::Nack(nack) => {
                //gestire nack
            }
            _ => {
                println!("Unhandled packet type");
            }
        }
    }

    fn send_message(&mut self, message: ServerMessages, route: Vec<NodeId>) {
        if let Some(&neighbour_id) = route.first() {
            if let Some(sender) = self.packet_send.get(&neighbour_id) {
                let serialized_data = bincode::serialize(&message).expect("Serialization failed");
                let packets = fragment_message(&serialized_data, route, 1);

                for mut packet in packets {
                    packet.routing_header.hop_index = 1;
                    sender.send(packet).expect("Errore durante l'invio del pacchetto al neighbour.");
                }
            } else {
                println!("Errore: Neighbour con NodeId {} non trovato!", neighbour_id);
            }
        } else {
            println!("Errore: Route vuota, impossibile determinare il neighbour!");
        }
    }

    fn update_graph(&mut self, path_trace: Vec<(NodeId, NodeType)>) {
        log::info!("Aggiornamento del grafo con i dati ricevuti: {:?}", path_trace);
        for i in 0..path_trace.len() - 1 {
            let (node_a, _) = path_trace[i];
            let (node_b, _) = path_trace[i + 1];
            self.topology.insert((node_a, node_b));
            self.topology.insert((node_b, node_a)); // Grafo bidirezionale
        }

        for (node_id, node_type) in path_trace {
            self.node_types.insert(node_id, node_type);
        }
    }

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

    fn reconstruct_message<T: DeserializeOwned>(&mut self, key: u64) -> Result<T, Box<dyn Error>> {
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
}

impl CommunicationServer {
    fn send_my_type(&mut self, client_id: NodeId) {
        if let Some(best_path) = self.compute_best_path(client_id) {
            self.send_message(ServerMessages::ServerType(self.clone().server_type), best_path);
        }
    }

    fn register_client(&mut self, client_id: NodeId) {
        if let Some(path) = self.compute_best_path(client_id) {
            if self.registered_client.contains(&client_id) {
                self.send_message(ServerMessages::RegistrationError("client already registered".to_string()), path);
            } else {
                self.registered_client.push(client_id.clone());
                self.send_message(ServerMessages::RegistrationOk, path);
            }
        }
    }

    fn send_register_client(&mut self, client_id: NodeId) {
        if let Some(hops) = self.compute_best_path(client_id) {
            let data = ServerMessages::ClientList(self.clone().registered_client);
            self.send_message(data, hops)
        }
    }

    fn forward_message(&mut self, target_id: NodeId, client_id: NodeId, message: String) {
        if let Some(hops) = self.compute_best_path(target_id) {
            let final_message = ServerMessages::MessageFrom(client_id, message);
            self.send_message(final_message, hops);
        }
    }

    fn add_neighbor(&mut self, node_id: NodeId, sender: Sender<Packet>) {
        if let std::collections::hash_map::Entry::Vacant(e) = self.packet_send.entry(node_id) {
            e.insert(sender);
            self.network_discovery()
        } else {
            panic!("Sender for node {node_id} already stored in the map!");
        }
    }
    fn remove_from_topology(&mut self, node_id: NodeId) {
        self.topology.retain(|&(a, b)| a != node_id && b != node_id);
        self.node_types.remove(&node_id);
    }
    fn remove_neighbor(&mut self, node_id: NodeId) {
        if self.packet_send.contains_key(&node_id) {
            self.packet_send.remove(&node_id);
            self.remove_from_topology(node_id);
            self.network_discovery();
        } else {
            panic!("the {} is not neighbour of the drone {}", node_id, self.id);
        }
    }
}
