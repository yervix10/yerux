# Proyectos y paquetes

`yerux new app` crea `app/yerux.toml`, `app/src/main.yx` y `app/tests/`.
Desde la raíz del proyecto, `yerux build` genera `target/<nombre>`,
`yerux run` compila y ejecuta el programa y `yerux test` ejecuta los bloques
de prueba de `tests/*.yx`.

## Declarar dependencias

```toml
[package]
name = "app"
version = "0.1.0"

[dependencies]
geometry = { path = "../geometry", version = "^1.2" }
parser = { git = "https://example.com/parser.git", version = ">=1.2, <2" }
lexer = { git = "https://example.com/lexer.git", tag = "v0.8.0" }
```

`geometry = "../geometry"` abrevia una dependencia de ruta sin requisito
de versión. Las rutas relativas, incluidas las de repositorios Git locales,
se interpretan desde el manifiesto que las declara. Una dependencia por ruta
de un paquete Git debe permanecer dentro de su checkout. Cada clave de
dependencia debe ser un identificador de Yerux y coincidir con el nombre
del paquete. `std` está reservado para la biblioteca estándar.

También puedes editar el manifiesto desde la línea de órdenes.

```sh
yerux add geometry --path ../geometry --version '^1.2'
yerux add parser --git https://example.com/parser.git --version '>=1.2, <2'
yerux add lexer --git https://example.com/lexer.git --tag v0.8.0
yerux remove lexer
```

`add` y `remove` resuelven el grafo propuesto antes de guardar. Un error de
resolución conserva el manifiesto y el lock existentes.

## Versiones y resolución

Las versiones siguen SemVer, con versiones preliminares y metadatos de
compilación. Los requisitos admiten `^`, `~`, comparaciones, comodines e
intersecciones separadas por comas. Por ejemplo, `^1.2` admite versiones
desde `1.2.0` hasta antes de `2.0.0`, y `~0.4` admite parches de `0.4`.
Para admitir una versión preliminar, el requisito debe mencionar una versión
preliminar con los mismos números mayor, menor y de parche.
[Sintaxis de requisitos de versión](https://docs.rs/semver/latest/semver/struct.VersionReq.html)

Una dependencia de ruta toma su versión del manifiesto. Para un requisito de
versión de Git, Yerux prueba etiquetas SemVer, con o sin prefijo `v`, de
mayor a menor. La etiqueta seleccionada debe coincidir con la versión del
manifiesto en esa revisión. Un `tag` explícito selecciona la etiqueta exacta.
Si la entrada incluye `tag` y `version`, también se comprueba el requisito
contra el manifiesto seleccionado. Una entrada Git sin selector prefiere
`HEAD`. El comando `yerux add NAME --git URL` sin selector graba la etiqueta
estable más alta de forma explícita cuando existe. Si el repositorio carece
de etiquetas de versión estables, fija el commit de `HEAD` en el lock.

Yerux resuelve dependencias transitivas y comparte una fuente y una versión
por nombre de paquete. Si una restricción posterior invalida una revisión Git,
prueba otra candidata compatible. Los conflictos de fuente o versión y los
ciclos producen errores. Las fuentes disponibles son rutas y repositorios Git.

## Importar módulos

`import geometry` busca `src/geometry.yx` en la dependencia declarada, con
`src/lib.yx` como alternativa. `import geometry.curves` carga
`src/curves.yx` del mismo paquete. Se aplican las reglas habituales de
visibilidad de `pub` y los alias de `import ... as ...`.

Un import local (`import util`) se busca en la carpeta del archivo que se
compila, `<root>` (SPEC.md, sección 6). En un proyecto el archivo principal es
`src/main.yx`, así que los imports locales se resuelven en `src/`. Un
`yerux run main.yx` desde la raíz del paquete no ve `src/`, y falla con
`cannot find module`. Los nombres de dependencias se resuelven con
`src/<nombre>.yx` y, si no existe, `src/lib.yx`; tests/spec_conformance.sh lo verifica.

El orden de búsqueda es: la biblioteca estándar (`std.*`), después una dependencia
declarada cuyo nombre sea el primer segmento (aunque exista un archivo local con ese
nombre) y por último un archivo local. Un archivo de `src/` busca en `src/`; el
archivo compilado fuera de `src/` busca en su propia carpeta y, si no está, en
`src/`. tests/packages.sh fija este orden.

Cada módulo puede importar módulos locales y dependencias directas de su
propio paquete. Para importar una dependencia transitiva desde la aplicación,
declárala también en el manifiesto de la aplicación. Los comandos que reciben
un archivo, como `yerux run src/main.yx`, buscan el `yerux.toml` más cercano
en los directorios superiores.

## Fijar y actualizar dependencias

Guarda `yerux.lock` en el control de versiones junto al manifiesto. Registra
el grafo completo, las versiones, las fuentes y los commits exactos de Git.
La resolución normal conserva las revisiones compatibles. Los checkouts
guardados en caché se pueden usar sin conexión. Las dependencias por ruta usan
los archivos actuales del disco, por lo que reproducir su contenido también
requiere conservar esos archivos.

- `yerux lock` resuelve el grafo y guarda el lock sin compilar.
- `yerux lock --update` resuelve de nuevo desde las fuentes configuradas y
  reconsidera las etiquetas que cumplen los requisitos.
- `yerux lock --locked` comprueba el lock existente. Usa `yerux build --locked`,
  `yerux run --locked` o `yerux test --locked` para exigirlo en CI. Si falta
  un checkout, Yerux intenta restaurarlo desde el commit que figura en el lock.
- Para migrar un lock antiguo sin versión de formato, ejecuta `yerux lock`.

Las dependencias Git requieren el comando `git`. Deja `.yerux/` y `target/`
fuera del control de versiones.
