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

## Dependencias desde la línea de órdenes

`yerux add NAME --path DIR` y `yerux add NAME --git URL --version "^1.2"` editan `[dependencies]`, resuelven la versión
(para Git, la etiqueta `vX.Y.Z` más alta que cumple el requisito) y actualizan `yerux.lock`, que ahora registra también
la versión resuelta de cada paquete. `yerux remove NAME` quita la dependencia. Un `version = "REQ"` en el manifiesto se
comprueba contra la versión del paquete dependiente en cada resolución. Detalle y sintaxis de requisitos
(`*`, `=`, `>=`, `^`, `~`): [TOOLS.md](TOOLS.md).
