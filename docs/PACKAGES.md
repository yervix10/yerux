# Proyectos y paquetes

Un proyecto tiene `yerux.toml` en la raíz y un punto de entrada en
`src/main.yx`. El manifiesto declara `[package]` (`name`, `version`) y
`[dependencies]`: una ruta relativa (`foo = { path = "../foo" }`) o un
repositorio (`foo = { git = "URL", tag = "v1" }`). Las rutas se resuelven
desde el manifiesto; Git se fija a la etiqueta indicada. La resolución escribe
`yerux.lock`, que registra origen y revisión/ruta resuelta.

`yerux new NAME` crea el esqueleto. Desde la raíz, `build` genera
`target/NAME`, `run` compila y ejecuta `src/main.yx`, y `test` descubre
los bloques de prueba de `tests/*.yx`. Dependencias se resuelven y fijan en
cada comando; módulos dependientes se importan por el mecanismo de módulos del
compilador.
