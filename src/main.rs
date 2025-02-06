mod communication_server;
mod content_server;

use std::fmt::Display;
use dronegowski_utils::hosts::{ServerMessages};
use dronegowski_utils::functions::generate_unique_id;
use wg_2024::network::{NodeId};
use wg_2024::packet::{NodeType, Packet};
use std::thread;
use serde::de::DeserializeOwned;
use crate::communication_server::CommunicationServer;
use crate::content_server::ContentServer;

pub trait DronegowskiServer {
    fn new(id: NodeId) -> Self;
    fn run(&mut self);
    fn handle_packet(&mut self, packet: Packet);
    fn send_message(&mut self, message: ServerMessages, route: Vec<NodeId>);
    fn update_graph(&mut self, path_trace: Vec<(NodeId, NodeType)>);
    fn compute_best_path(&self, target_client: NodeId) -> Option<Vec<NodeId>>;
    fn reconstruct_message<T: DeserializeOwned>(&mut self, key: u64) -> Result<T, Box<dyn std::error::Error>>;
}


fn main() {
    // creazione del Server
    let mut communication_server = CommunicationServer::new(generate_unique_id() as NodeId);

    let mut content_server = ContentServer::new(generate_unique_id() as NodeId);

    let mut handles = Vec::new();

    handles.push(thread::spawn(move || communication_server.run()));
}