use std::collections::{HashMap, HashSet};
use std::fmt::Display;
use crossbeam_channel::{select, select_biased, unbounded, Receiver, Sender};
use dronegowski_utils::functions::{fragment_message, fragmenter};
use dronegowski_utils::hosts::{ServerType, ServerCommand, ServerEvent, TestMessage};
use serde::{Serialize, Serializer};
use wg_2024::config::{Server};
use wg_2024::controller::{DroneCommand, DroneEvent};
use wg_2024::network::{NodeId, SourceRoutingHeader};
use wg_2024::packet::{FloodRequest, NodeType, Packet, PacketType};
use std::fs::File;
use std::thread;
use std::time::Duration;
use serde::ser::SerializeStruct;
// use eframe::egui;
// use log::LevelFilter;
// use simplelog::{ConfigBuilder, WriteLogger};

struct DronegowskiServer {
    id: NodeId,
    sim_controller_send: Sender<ServerEvent>, //Channel used to send commands to the SC
    sim_controller_recv: Receiver<ServerCommand>, //Channel used to receive commands from the SC
    packet_send: HashMap<NodeId, Sender<Packet>>, //Map containing the sending channels of neighbour nodes
    packet_recv: Receiver<Packet>,           //Channel used to receive packets from nodes
    server_type: ServerType,
    topology: HashSet<(NodeId, NodeId)>, // Edges of the graph
    node_types: HashMap<NodeId, NodeType>, // Node types (Client, Drone, Server)
}

impl DronegowskiServer {
    fn new(id: NodeId, scs: Sender<ServerEvent>, scr: Receiver<ServerCommand>, ps: HashMap<NodeId, Sender<Packet>>, pr: Receiver<Packet>, st: ServerType) -> DronegowskiServer {
        DronegowskiServer {
            id,
            sim_controller_send: scs,
            sim_controller_recv: scr,
            packet_send: ps,
            packet_recv: pr,
            server_type: st,
            topology: HashSet::new(),
            node_types: HashMap::new(),
        }
    }

    fn run(&mut self) {
        loop {
            select_biased! {
                recv(self.packet_recv) -> packet_res => {
                    if let Ok(packet) = packet_res {
                        self.handle_packet(packet);
                    }
                }

                recv(self.sim_controller_recv) -> command_res => {
                    if let Ok(cmd) = command_res {
                        match cmd {
                            ServerCommand::AddClient(client_id) => self.register_client(client_id),
                            ServerCommand::SendClients(client_id) => self.send_register_client(client_id),
                            ServerCommand::SendMessage(client_message) => self.forward_message(client_message)
                        }
                    }
                }
            }
        }
    }

    fn register_client(&mut self, client_id: NodeId) {
        match self.clone().server_type {
            ServerType::CommunicationServer(mut registered_client) => {
                registered_client.push(client_id.clone());
            },
            _ => {
                println!("you can't do this");
            }
        }
    }

    fn find_path(&self, client_id: &NodeId) -> Vec<NodeId> {

        // Creazione del pacchetto FloodRequest
        let packet = Packet {
            pack_type: PacketType::FloodRequest(FloodRequest {
                flood_id: 123,
                initiator_id: self.id, // ID del server come iniziatore
                path_trace: vec![(self.id, NodeType::Server)], // Traccia iniziale
            }),
            routing_header: SourceRoutingHeader {
                hop_index: 0,
                hops: vec![],
            },
            session_id: 1,
        };

        // Invio del pacchetto a tutti i vicini
        for (&neighbour_id, sender) in &self.packet_send {
            sender
                .send(packet.clone())
                .expect(&format!("Error sending packet to neighbour {}", neighbour_id));
        }

        // Variabile per raccogliere le informazioni ricevute
        let mut responses = Vec::new();

        match self.packet_recv.recv() {
            Ok(received_packet) => {
                if let PacketType::FloodResponse(response) = received_packet.pack_type {
                    responses.push((response.path_trace.first().unwrap().0, response));
                }
            }
            Err(_) => {
                println!("Timeout or no response received from neighbour");
            }
        }

        let mut paths = Vec::new();

        // Elaborazione delle risposte raccolte
        if responses.is_empty() {
            println!("No valid FloodResponses received.");
        } else {
            for (_, response) in responses {
                paths.push(response.path_trace);
            }
        }

        let mut possible_neighbours:Vec<Vec<NodeId>> = Vec::new();

        for path in paths.iter() {
            if path.iter().any(|x| x.0 == *client_id) {
                possible_neighbours.push(path.iter().map(|node| node.0.clone()).collect::<Vec<_>>());
            }
        }

        let best_path = compute_best_path(&possible_neighbours, self.id, client_id);

        best_path
    }

    fn send_register_client(&mut self, client_id: &NodeId) { // TESTARE!!!!
        let hops = self.find_path(client_id);
        let neighbour_id = hops.first().unwrap();

        let data: TestMessage;

        match self.clone().server_type {
            ServerType::CommunicationServer(RegisteredClient) => data = TestMessage::Vector(RegisteredClient),
            ServerType::ContentServer => {

            }
        }

        let packets = fragment_message(&data, hops.clone(), 1);

        for mut packet in packets {
            // Invia il pacchetto al neighbour utilizzando il suo NodeId
            if let Some(sender) = self.packet_send.get(&neighbour_id) {
                packet.routing_header.hop_index = 1;
                sender.send(packet).expect("Errore durante l'invio del pacchetto al neighbour.");
            } else {
                println!("Errore: Neighbour con NodeId {} non trovato!", neighbour_id);
            }
        }
    }

    fn forward_message(&mut self, message: TestMessage) {
        if let TestMessage::WebServerMessages(ref client_message) = message {
            let client_id = client_message[0].id;
            let hops = self.find_path(&client_id);
            let neighbour_id = hops.first().unwrap();

            let packets = fragment_message(&message, hops.clone(), 1);

            for packet in packets {
                // Invia il pacchetto al neighbour utilizzando il suo NodeId
                if let Some(sender) = self.packet_send.get(&neighbour_id) {
                    sender.send(packet).expect("Errore durante l'invio del pacchetto al neighbour.");
                    println!("Pacchetto inviato al neighbour con NodeId {}", neighbour_id);
                } else {
                    println!("Errore: Neighbour con NodeId {} non trovato!", neighbour_id);
                }
            }
        }
    }
}

fn compute_best_path(paths: &Vec<Vec<NodeId>>, p1: NodeId, p2: &NodeId) -> Vec<NodeId> {
    todo!()
}

fn main() {
    // Creazione dei canali
    let (sim_controller_send, sim_controller_recv) = unbounded::<ServerEvent>();
    let (send_controller, controller_recv) = unbounded::<ServerCommand>();
    let (packet_send, packet_recv) = unbounded::<Packet>();

    // Mappa dei vicini (drone collegati)
    let (neighbor_send, neighbor_recv) = unbounded();
    let mut senders = HashMap::new();
    senders.insert(2, neighbor_send); // Drone 2 come vicino

    // creazione del Server
    let mut server = DronegowskiServer::new(
        1,
        sim_controller_send,
        controller_recv,
        senders.clone(),
        packet_recv.clone(),
        ServerType::CommunicationServer(Vec::new())
    );

    let mut handles = Vec::new();

    handles.push(thread::spawn(move || server.run()));
}