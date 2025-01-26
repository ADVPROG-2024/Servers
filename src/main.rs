use std::collections::HashMap;
use std::fmt::Display;
use crossbeam_channel::unbounded;
use dronegowski_utils::functions::{fragment_message, fragmenter};
use serde::{Serialize, Serializer};
use wg_2024::config::{Client, Server, Drone};
use wg_2024::controller::{DroneCommand, DroneEvent};
use wg_2024::network::{NodeId, SourceRoutingHeader};
use wg_2024::packet::{FloodRequest, NodeType, Packet, PacketType};
use std::fs::File;
use std::thread;
use std::time::Duration;
// use eframe::egui;
// use log::LevelFilter;
// use simplelog::{ConfigBuilder, WriteLogger};

enum ServerMessageType {
    RegisteredClient(Vec<Client>),
    Text(String, NodeId),
}

#[derive(Serialize)]
struct  ServerMessage {
    message: ServerMessageType
}

#[derive(Serialize)]
struct CommunicationServer {
    server: Server,
    registered_client: Vec<Client>,
}

impl CommunicationServer {
    fn new() -> CommunicationServer {
        CommunicationServer {
            server: Server {
                id: 1,
                connected_drone_ids: Vec::new(),
            },
            registered_client: Vec::new(),
        }
    }

    fn register_client(&mut self, client_id: NodeId) {
        self.server.connected_drone_ids.push(client_id.clone());
    }

    fn find_path(&self, client_id: NodeId) -> Vec<NodeId> {
        // Creazione dei canali per ciascun neighbour
        let mut senders = HashMap::new();

        for &neighbour_id in &self.server.connected_drone_ids {
            let (sender, receiver) = unbounded::<Packet>();
            senders.insert(neighbour_id, sender);
        }

        // Creazione del pacchetto FloodRequest
        let packet = Packet {
            pack_type: PacketType::FloodRequest(FloodRequest {
                flood_id: 123,
                initiator_id: self.server.id, // ID del server come iniziatore
                path_trace: vec![(self.server.id, NodeType::Server)], // Traccia iniziale
            }),
            routing_header: SourceRoutingHeader {
                hop_index: 0,
                hops: vec![],
            },
            session_id: 1,
        };

        // Invio del pacchetto a tutti i vicini
        for (&neighbour_id, sender) in &senders {
            sender
                .send(packet.clone())
                .expect(&format!("Error sending packet to neighbour {}", neighbour_id));
        }


        let mut paths:Vec<Vec<NodeId>> = Vec::new();

        // ricevere le flooding response
        // aggiungere tutti i path_race al vettore paths

        let mut possible_neighbours:Vec<Vec<NodeId>> = Vec::new();

        for path in paths.iter() {
            if path.iter().any(|x| x == *client_id) {
                possible_neighbours.push(path.clone());
            }
        }

        let best_path = compute_best_path(&possible_neighbours, self.server.id, client_id);

        best_path
    }

    fn send_register_drone(&mut self, client_id: NodeId) { // TESTARE!!!!
        let hops = self.find_path(client_id);
        let neighbour_id = hops.first().unwrap();
        // Creazione del canale per il neighbor
        let (neighbor_send, neighbor_recv) = crossbeam_channel::unbounded::<Packet>();

        // Crea una mappa per i neighbors
        let mut senders = HashMap::new();
        senders.insert(neighbour_id, neighbor_send.clone());

        let data = ServerMessage {
            message: ServerMessageType::RegisteredClient(self.registered_client.clone()),
        };

        let packets = fragment_message(&data, hops, 1);

        for packet in packets {
            // Invia il pacchetto al neighbour utilizzando il suo NodeId
            if let Some(sender) = senders.get(&neighbour_id) {
                sender.send(packet).expect("Errore durante l'invio del pacchetto al neighbour.");
                println!("Pacchetto inviato al neighbour con NodeId {}", neighbour_id);
            } else {
                println!("Errore: Neighbour con NodeId {} non trovato!", neighbour_id);
            }
        }

        //il check va fatto così oppure lo faccio tramite Ack/Nack
        // Ricezione simulata per verificare che il neighbour abbia ricevuto il pacchetto
        match neighbor_recv.recv() {
            Ok(received_packet) => {
                println!("Neighbour con NodeId {} ha ricevuto il pacchetto: {:?}", neighbour_id, received_packet);
            }
            Err(_) => {
                println!("Errore: il neighbour non ha ricevuto alcun pacchetto.");
            }
        }
    }

    fn forward_message(&mut self, client_id: NodeId, message: ServerMessage) { //non so se ha senso far passare l'intero messaggio oppure il singolo pacchetto
        let hops = self.find_path(client_id);
        let neighbour_id = hops.first().unwrap();
        // Creazione del canale per il neighbor
        let (neighbor_send, neighbor_recv) = crossbeam_channel::unbounded::<Packet>();

        // Crea una mappa per i neighbors
        let mut senders = HashMap::new();
        senders.insert(neighbour_id, neighbor_send.clone());

        let packets = fragment_message(&message, hops, 1);

        for packet in packets {
            // Invia il pacchetto al neighbour utilizzando il suo NodeId
            if let Some(sender) = senders.get(&neighbour_id) {
                sender.send(packet).expect("Errore durante l'invio del pacchetto al neighbour.");
                println!("Pacchetto inviato al neighbour con NodeId {}", neighbour_id);
            } else {
                println!("Errore: Neighbour con NodeId {} non trovato!", neighbour_id);
            }
        }
    }
}

fn compute_best_path(paths: &Vec<Vec<NodeId>>, p1: NodeId, p2: NodeId) -> Vec<NodeId> {
    todo!()
}

fn main() {

}