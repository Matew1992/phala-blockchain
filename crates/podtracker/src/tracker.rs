use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use docker_api::{
    api::{container::opts::ContainerCreateOpts, PublishPort},
    Docker,
};
use serde::Serialize;

type Uuid = String;

#[derive(Clone, Serialize)]
pub struct Pod {
    pub image: String,
    pub container_id: String,
    pub uuid: String,
    pub tcp_ports: Vec<u16>,
}

pub struct Tracker {
    docker: Docker,
    pods: HashMap<Uuid, Pod>,
    available_tcp_ports: Vec<u16>,
}

impl Tracker {
    pub fn new(docker: Docker, (port_range_from, port_range_to): (u16, u16)) -> Tracker {
        Tracker {
            docker: docker,
            pods: Default::default(),
            available_tcp_ports: (port_range_from..port_range_to).collect(),
        }
    }

    pub async fn create_pod(&mut self, image: &str, uuid: &str) -> Result<Pod> {
        // TODO.kevin.must: get the port from somthing like manifest.json
        let required_ports = vec![80];
        let exposed_ports = self
            .allocate_tcp_ports(required_ports.len())
            .ok_or(anyhow::anyhow!("no available ports"))?;
        let mut builder = ContainerCreateOpts::builder(image).auto_remove(true);
        for (po, pi) in exposed_ports.iter().zip(required_ports.iter()) {
            builder = builder.expose(PublishPort::tcp(*po as _), *pi);
        }
        let opts = builder.build();
        let contrainer = self.docker.containers().create(&opts).await?;
        let pod = Pod {
            image: image.to_owned(),
            uuid: uuid.to_owned(),
            container_id: contrainer.id().to_owned(),
            tcp_ports: exposed_ports,
        };
        self.pods.insert(uuid.to_owned(), pod.clone());
        Ok(pod)
    }

    pub async fn stop_pod(&mut self, uuid: &str) -> Result<()> {
        let pod = self
            .pods
            .get(uuid)
            .ok_or(anyhow::anyhow!("Pod {} not found", uuid))?;
        let contrainer = self.docker.containers().get(pod.container_id.as_str());
        let wait = Duration::from_secs(5);
        contrainer.stop(Some(wait)).await?;
        if let Some(pod) = self.pods.remove(uuid) {
            self.free_tcp_ports(&pod.tcp_ports);
        }
        Ok(())
    }
}

impl Tracker {
    fn allocate_tcp_ports(&mut self, n: usize) -> Option<Vec<u16>> {
        if n > self.available_tcp_ports.len() {
            return None;
        }
        Some(self.available_tcp_ports.drain(0..n).collect())
    }

    fn free_tcp_ports(&mut self, ports: &[u16]) {
        self.available_tcp_ports.extend(ports);
    }
}
