use std::collections::HashMap;
use std::fmt::Display;
use dronegowski_utils::hosts::{ServerCommand, ServerEvent, ServerMessages, ServerType};
use dronegowski_utils::functions::generate_unique_id;
use wg_2024::network::{NodeId};
use wg_2024::packet::{NodeType, Packet};
use std::thread;
use crossbeam_channel::{unbounded, Receiver, Sender};
use serde::de::DeserializeOwned;
use crate::communication_server::CommunicationServer;
use crate::ContentServer;
//use crate::content_server::ContentServer;

pub trait DronegowskiServer {
    //fn new(id: NodeId, sim_controller_send: Sender<ServerEvent>, sim_controller_recv: Receiver<ServerCommand>, packet_recv: Receiver<Packet>, packet_send: HashMap<NodeId, Sender<Packet>>, server_type: ServerType) -> Self;
    fn network_discovery(&self);
    fn run(&mut self);
    fn handle_packet(&mut self, packet: Packet);
    fn send_message(&mut self, message: ServerMessages, route: Vec<NodeId>);
    fn update_graph(&mut self, path_trace: Vec<(NodeId, NodeType)>);
    fn compute_best_path(&self, target_client: NodeId) -> Option<Vec<NodeId>>;
    fn reconstruct_message<T: DeserializeOwned>(&mut self, key: u64) -> Result<T, Box<dyn std::error::Error>>;
}

#[test]
fn main(){
    let (sim_controller_send, sim_controller_recv) = unbounded:: <ServerEvent>();
    let (send_controller, controller_recv) = unbounded::<ServerCommand>() ;
    let (packet_send, packet_recv) = unbounded::<Packet>();

    let mut neighbours:HashMap<NodeId, Sender<Packet>> = HashMap::new(); // Packet Send Server (canali dei nodi vicini a cui può inviare i pacchetti il server)


    let mut handles = Vec::new();

    handles.push(thread::spawn(move || {
        let mut content_server = ContentServer::new(1, sim_controller_send, controller_recv, packet_recv, neighbours, ServerType::Content, "src/files", "src/medias");
        content_server.run();
    }));
}