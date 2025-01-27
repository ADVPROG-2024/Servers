use std::collections::{HashMap, HashSet};
use std::fmt::Display;
use crossbeam_channel::{select, select_biased, unbounded, Receiver, Sender};
use dronegowski_utils::functions::{fragment_message, fragmenter};
use dronegowski_utils::hosts::{ServerType, ServerCommand, ServerEvent, TestMessage};
use wg_2024::network::{NodeId};
use wg_2024::packet::{FloodRequest, NodeType, Packet, PacketType};
use std::fs::File;
use std::thread;
use std::time::Duration;
// use eframe::egui;
// use log::LevelFilter;
// use simplelog::{ConfigBuilder, WriteLogger};

struct DronegowskiServer {
    id: NodeId,
    sim_controller_send: Sender<ServerEvent>, //Channel used to send commands to the SC
    sim_controller_recv: Receiver<ServerCommand>, //Channel used to receive commands from the SC
    packet_send: HashMap<NodeId, Sender<Packet>>, //Map containing the sending channels of neighbour nodes
    packet_recv: Receiver<Packet>,           //Channel used to receive packets from nodes
    message_storage: HashMap<(usize, NodeId), (Vec<u8>, Vec<bool>)>, // Store for reassembling messages
    server_type: ServerType,
    topology: HashSet<(NodeId, NodeId)>, // Edges of the graph
    node_types: HashMap<NodeId, NodeType>, // Node types (Client, Drone, Server)
}

impl DronegowskiServer {
    fn new(id: NodeId, scs: Sender<ServerEvent>, scr: Receiver<ServerCommand>, ps: HashMap<NodeId, Sender<Packet>>, pr: Receiver<Packet>,ms:HashMap<(usize, NodeId), (Vec<u8>, Vec<bool>)>,  st: ServerType) -> DronegowskiServer {
        DronegowskiServer {
            id,
            sim_controller_send: scs,
            sim_controller_recv: scr,
            packet_send: ps,
            packet_recv: pr,
            message_storage: ms,
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
                            /*ServerCommand::SendClients(client_id) => self.send_register_client(client_id),
                            ServerCommand::SendMessage(client_message) => self.forward_message(client_message)

                            scusa pg ti ho commentato ste due righe, ma davano errore. penso in seguito a modifiche ad altri file
                            ho aggiunto queste righe fittizie per non fare uscire errori per adesso
                            */
                            ServerCommand::SendClients(..)=>{},
                            ServerCommand::SendMessage(..)=>{}
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

    fn send_register_client(&mut self, client_id: &NodeId) { // TESTARE!!!!
        if let ServerType::CommunicationServer(registered_clients) = self.clone().server_type {
            if let Some(hops) = self.compute_best_path(client_id) {
                let neighbour_id = hops.first().unwrap();

                let data = TestMessage::Vector(registered_clients);

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
        }
    }

    fn forward_message(&mut self, message: TestMessage) {
        if let TestMessage::WebServerMessages(ref client_message) = message {
            let client_id = client_message[0].id;
            if let Some(hops) = self.compute_best_path(&client_id) {
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

    fn handle_packet(&mut self, packet: Packet) {
        match packet.pack_type {
            PacketType::MsgFragment(fragment) => {
                //assemblare il messaggio
            },
            PacketType::FloodResponse(fs) => {
                //aggiornare la topologia in base alla flood_response
            },
            PacketType::FloodRequest(fr) => {
                //gestire la fr
            }
            _ =>{
                //gestire gli altri tipi di pacchetti (ack, nack). Distinguere per i vari tipi di Server o è possibile implementazione unica?
            }
        }
    }

    fn handle_packet_text(&mut self, packet: Packet) {

    }

    fn handle_packet_media(&mut self, packet: Packet) {

    }

    fn compute_best_path(&self, target_client: &NodeId) -> Option<Vec<NodeId>> {
        use std::collections::VecDeque;

        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        let mut predecessors = HashMap::new();

        queue.push_back(self.id);
        visited.insert(self.id);

        while let Some(current) = queue.pop_front() {
            if current == *target_client {
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
        HashMap::new(),
        ServerType::CommunicationServer(Vec::new())
    );

    let mut handles = Vec::new();

    handles.push(thread::spawn(move || server.run()));
}