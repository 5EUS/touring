use wasmtime_wasi::{WasiCtx, WasiView};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpView};

// Host context with WASI and HTTP support
pub(crate) struct Host {
    pub(crate) wasi: WasiCtx,
    pub(crate) table: wasmtime_wasi::ResourceTable,
    pub(crate) http: WasiHttpCtx, // TODO limit allowed hosts by plugin config
}

impl WasiView for Host {
    fn ctx(&mut self) -> &mut WasiCtx { &mut self.wasi }
    fn table(&mut self) -> &mut wasmtime_wasi::ResourceTable { &mut self.table }
}

impl WasiHttpView for Host {
    fn ctx(&mut self) -> &mut WasiHttpCtx { &mut self.http }
    fn table(&mut self) -> &mut wasmtime_wasi::ResourceTable { &mut self.table }
}
