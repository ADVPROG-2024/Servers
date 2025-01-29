mod communication_server;
mod content_server;

use std::collections::{HashMap, HashSet};
use std::fmt::Display;
use crossbeam_channel::{select, select_biased, unbounded, Receiver, Sender};
use dronegowski_utils::functions::{assembler, deserialize_message, fragment_message, fragmenter};
use dronegowski_utils::hosts::{ServerType, ServerCommand, ServerEvent, TestMessage, ClientMessages, ServerMessages, TextServer};
use dronegowski_utils::functions::generate_unique_id;
use wg_2024::network::{NodeId};
use wg_2024::packet::{FloodRequest, Fragment, NodeType, Packet, PacketType};
use std::fs::File;
use std::thread;
use std::time::Duration;
use dronegowski_utils::hosts::ServerType::Communication;
use serde::de::DeserializeOwned;
use crate::communication_server::CommunicationServer;

pub trait DronegowskiServer {
    fn new(id: NodeId) -> Self;
    fn run(&mut self);
    fn handle_packet(&mut self, packet: Packet);
    fn send_message(&mut self, message: ServerMessages, route: Vec<NodeId>);
    fn send_my_type(&mut self, client_id: NodeId);
    fn update_graph(&mut self, path_trace: Vec<(NodeId, NodeType)>);
    fn compute_best_path(&self, target_client: NodeId) -> Option<Vec<NodeId>>;
    fn reconstruct_message<T: DeserializeOwned>(&mut self, key: u64) -> Result<T, Box<dyn std::error::Error>>;

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
    let mut communication_server = CommunicationServer::new(
        generate_unique_id() as NodeId
    );

    let mut handles = Vec::new();

    handles.push(thread::spawn(move || communication_server.run()));
}