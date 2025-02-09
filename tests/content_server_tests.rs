#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use crossbeam_channel::unbounded;
    use dronegowski_utils::hosts::{ServerCommand, ServerEvent, ServerType};
    use wg_2024::packet::Packet;
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

    fn test_flood_response(){
        let (sim_controller_send, sim_controller_recv) = unbounded:: <ServerEvent>();
        let (send_controller, controller_recv) = unbounded::<ServerCommand>() ;
        let (packet_send, packet_recv) = unbounded::<Packet>();
        let mut senders = HashMap::new();

        let content_server = ContentServer::new(6, sim_controller_send, controller_recv, packet_recv, senders, ServerType::Content, "src/files", "src/medias");

        //continuare da qua, devo pushare adesso
    }
}
