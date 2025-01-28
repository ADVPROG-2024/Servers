use std::collections::{HashMap, HashSet};
use std::fmt::Display;
use crossbeam_channel::{select, select_biased, unbounded, Receiver, Sender};
use dronegowski_utils::functions::{assembler, deserialize_message, fragment_message, fragmenter};
use dronegowski_utils::hosts::{ServerType, ServerCommand, ServerEvent, TestMessage, ClientMessages};
use wg_2024::network::{NodeId};
use wg_2024::packet::{FloodRequest, Fragment, NodeType, Packet, PacketType};
use std::fs::File;
use std::thread;
use std::time::Duration;
use serde::Deserialize;

#[derive(Clone)]
struct DronegowskiServer {
    id: NodeId,
    sim_controller_send: Sender<ServerEvent>,      //Channel used to send commands to the SC
    sim_controller_recv: Receiver<ServerCommand>,  //Channel used to receive commands from the SC
    packet_send: HashMap<NodeId, Sender<Packet>>,  //Map containing the sending channels of neighbour nodes
    packet_recv: Receiver<Packet>,                 //Channel used to receive packets from nodes
    server_type: ServerType,                       //typology of the server
    topology: HashSet<(NodeId, NodeId)>,           // Edges of the graph
    node_types: HashMap<NodeId, NodeType>,         // Node types (Client, Drone, Server)
    message_storage: Vec<Fragment>,                // Store for reassembling messages
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
            message_storage: Vec::new(),
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
            }
        }
    }

    fn handle_packet(&mut self, packet: Packet) {
        let client_id = packet.routing_header.hops[0];
        match packet.pack_type {
            PacketType::MsgFragment(fragment) => {
                self.message_storage.push(fragment.clone());
                if fragment.fragment_index == fragment.total_n_fragments {
                    match self.reconstruct_message() {
                        Ok(message) => {
                            match message {
                                TestMessage::WebServerMessages(client_message) => {
                                    match client_message {
                                        ClientMessages::ServerType => {

                                        }
                                        ClientMessages::RegistrationToChat => self.register_client(client_id),
                                        ClientMessages::ClientList => self.send_register_client(client_id),
                                        ClientMessages::MessageFor(target_id, message) => self.forward_message(target_id, message),
                                        _ => {}
                                    }
                                }
                            }
                        },
                        Err(e) => {
                            println!("Error reconstruct the message: {}", e);
                        }
                    }
                }
            },
            PacketType::FloodResponse(flood_response) => {
                self.update_graph(flood_response.path_trace);
            }
            _ => {}
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

    fn send_register_client(&mut self, client_id: NodeId) { // TESTARE!!!!
        if let ServerType::CommunicationServer(registered_clients) = self.clone().server_type {
            if let Some(hops) = self.compute_best_path(client_id) {
                let neighbour_id = hops.first().unwrap();

                let data = TestMessage::Vector(registered_clients);
                let serialized_data = bincode::serialize(&data).expect("Serialization failed");

                let packets = fragment_message(&serialized_data, hops.clone(), 1);

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

    fn forward_message(&mut self, target_id: NodeId, message: String) {
        if let Some(hops) = self.compute_best_path(target_id) {
            let neighbour_id = hops.first().unwrap();

            let serialized_message = bincode::serialize(&message).expect("Serialization failed");
            let packets = fragment_message(&serialized_message, hops.clone(), 1);

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

    pub fn reconstruct_message<'a, T: Deserialize<'a>>(&self) -> Result<T, Box<dyn std::error::Error>> {
        // Determina il numero totale di frammenti
        let total_fragments = self.message_storage
            .get(0)
            .ok_or("Nessun frammento fornito")?
            .total_n_fragments;

        // Ordina i frammenti in base al loro fragment_index
        let mut fragment_map = HashMap::new();
        for fragment in self.clone().message_storage {
            fragment_map.insert(fragment.fragment_index, fragment);
        }

        // Verifica che tutti i frammenti siano presenti
        if fragment_map.len() as u64 != total_fragments {
            return Err("Non tutti i frammenti sono stati ricevuti".into());
        }

        // Inizializza il buffer per i dati completi
        let mut full_data = Vec::new();

        // Usa l'assembler per ogni frammento in ordine
        for index in 0..total_fragments {
            if let Some(fragment) = fragment_map.get(&index) {
                assembler(&mut full_data, fragment);
            } else {
                return Err("Frammento mancante".into());
            }
        }

        // Deserializza il messaggio completo
        let message: T = bincode::deserialize(&full_data)?;
        Ok(message)
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
        ServerType::CommunicationServer(Vec::new())
    );

    let mut handles = Vec::new();

    handles.push(thread::spawn(move || server.run()));
}