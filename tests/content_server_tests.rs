#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use crossbeam_channel::unbounded;
    use dronegowski_utils::hosts::{ServerCommand, ServerEvent, ServerType};
    use wg_2024::network::SourceRoutingHeader;
    use wg_2024::packet::{FloodResponse, NodeType, Packet, PacketType};
    use wg_2024::packet::PacketType::FloodRequest;
    use servers::{ContentServer, MediaServer, TextServer};
    use super::*;

    #[test]
    fn test_text_server() {
        let text_server = TextServer::new_from_folder("src/files");
        let files_list = text_server.list_files();

        assert_eq!(files_list.len(), 1);
        println!("{:#?}", files_list);

        let file_content = text_server.get_file_text(files_list[0].0).unwrap();
        println!("File[0]: {:?}", file_content);
    }

    #[test]
    fn test_media_server() {
        let media_server = MediaServer::new_from_folder("src/medias");
        println!("{:#?}", media_server.stored_media.keys());

        let media0 = media_server.stored_media.get(&0);
        println!("{:#?}", media0);
    }

    //#[test]           IL TEST FUNZIONA CORRETTAMENTE MA NEL NETWORK_INITIALIZER NON MI SEMBRA FUNZIONARE CORRETTAMENTE
    // fn test_flood_response(){
    //     let (sim_controller_send, sim_controller_recv) = unbounded:: <ServerEvent>();
    //     let (send_controller, controller_recv) = unbounded::<ServerCommand>() ;
    //     let (packet_send, packet_recv) = unbounded::<Packet>();
    //     let mut senders = HashMap::new();
    //
    //     let content_server = ContentServer::new(6, sim_controller_send, controller_recv, packet_recv, senders, ServerType::Content, "src/files", "src/medias");
    //
    //     //continuare da qua, devo pushare adesso
    //
    //     //ricevuto
    //     let flood_request  = wg_2024::packet::FloodRequest{flood_id: 1739095502690, initiator_id: 4, path_trace: vec![(4, NodeType::Client), (2, NodeType::Drone)]};
    //
    //     let mut response_path_trace = flood_request.path_trace.clone();
    //     response_path_trace.push((content_server.id, NodeType::Server));
    //
    //     let flood_response = FloodResponse {
    //         flood_id: flood_request.flood_id,
    //         path_trace: response_path_trace,
    //     };
    //
    //     let response_packet = Packet {
    //         pack_type: PacketType::FloodResponse(flood_response),
    //         routing_header: SourceRoutingHeader {
    //             hop_index: 0,
    //             hops: flood_request.path_trace.iter().rev().map(|(id, _)| *id).collect(),
    //         },
    //         session_id: 1739095502690,
    //     };
    //
    //     println!("ContentServer {}: Sending FloodResponse: {:?}", content_server.id, response_packet);
    //     let next_node = response_packet.routing_header.hops[0];
    //     println!("{:#?}", next_node);
    //     //dovrei mandare
    //     // Packet { routing_header: SourceRoutingHeader { hop_index: 0, hops: [2, 4] }, session_id: 1739095502690, pack_type: FloodResponse(FloodResponse { flood_id: 1739095502690, path_trace: [(4, Client), (2, Drone), (6, Server)] }) }
    // }
}
