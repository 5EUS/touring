pub mod wasmsource
{
    use anyhow::Result;
    use wasmtime::{Engine, Module, Store, Linker, Instance};

    pub struct WasmSource {
        instance: Instance,
        store: Store<()>,
    }

    impl WasmSource {
        pub fn new(wasm_path: &str) -> Result<Self> {
            let engine = Engine::default();
            let module = Module::from_file(&engine, wasm_path)?;
            let mut store = Store::new(&engine, ());
            let linker = Linker::new(&engine);
            let instance = linker.instantiate(&mut store, &module)?;
            Ok(Self { instance, store })
        }
        
        pub fn call_fetch_list(&mut self, query: &str) -> Result<String> {
            // Get the memory export from the instance
            let memory = self
                .instance
                .get_memory(&mut self.store, "memory")
                .ok_or_else(|| anyhow::anyhow!("Failed to find memory export"))?;

            // Allocate memory for the query string
            let query_bytes = query.as_bytes();
            let query_len = query_bytes.len() as i32;

            // Assume the WebAssembly module has an `alloc` function to allocate memory
            let alloc_func = self
                .instance
                .get_typed_func::<i32, i32>(&mut self.store, "alloc")?;
            let query_ptr = alloc_func.call(&mut self.store, query_len)?;

            // Write the query string into the allocated memory
            memory.write(&mut self.store, query_ptr as usize, query_bytes)?;

            // Call the `fetch_list` function with the query pointer and length
            let fetch_list_func = self
                .instance
                .get_typed_func::<(i32, i32), i32>(&mut self.store, "fetch_list")?;
            let result_ptr = fetch_list_func.call(&mut self.store, (query_ptr, query_len))?;

            // Read the result string from memory
            let mut result_buffer = vec![0u8; 1024]; // Assume a max result size of 1024 bytes
            memory.read(&mut self.store, result_ptr as usize, &mut result_buffer)?;

            // Convert the result buffer to a string
            let result = String::from_utf8(result_buffer)?.trim_end_matches('\0').to_string();

            Ok(result)
        }

    }
}
