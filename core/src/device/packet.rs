use std::{future::Future, io, pin::Pin};

pub type PacketDeviceFuture<'a, T> = Pin<Box<dyn Future<Output = io::Result<T>> + Send + 'a>>;

pub trait PacketDevice: Send + Sync + 'static {
    fn recv<'a>(&'a self, buf: &'a mut [u8]) -> PacketDeviceFuture<'a, usize>;

    fn send<'a>(&'a self, packet: &'a [u8]) -> PacketDeviceFuture<'a, usize>;
}

impl PacketDevice for tun_rs::AsyncDevice {
    fn recv<'a>(&'a self, buf: &'a mut [u8]) -> PacketDeviceFuture<'a, usize> {
        Box::pin(async move { tun_rs::AsyncDevice::recv(self, buf).await })
    }

    fn send<'a>(&'a self, packet: &'a [u8]) -> PacketDeviceFuture<'a, usize> {
        Box::pin(async move { tun_rs::AsyncDevice::send(self, packet).await })
    }
}
