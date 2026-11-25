# Plan de rendimiento (rama `amd-optimized`)

Máquina: RX 7900 XTX 24 GB, Ryzen 7 5800X3D, 64 GB. Escena de medición: la celda
del prólogo, Lucia quieta, últimos 60 s de una corrida de 330 s.

## Punto de partida

| | base (W0, Z0) | hoy (AD1) |
|---|---|---|
| cuadro | 213-218 ms | 167 ms (6 fps) |
| hilo que envía, preparando | 125 ms | 115 ms |
| de eso, copiar memoria | 75 ms | 73 ms |
| GPU | 77 ms | 35 ms |
| hilo que trae resultados (en paralelo) | 104 ms | 94 ms |

AD1 es una sola corrida (AD0, en frío: 180 ms). El límite es la CPU, en el hilo que
envía. Números de AD1 por cuadro, sumando hilos: 13.554 copias del juego para solo
6,6 MB (35 ms en llamadas al núcleo); 2.447 preguntas de "¿qué cambió?" sobre 9,0 GB,
98 % sin cambios (69 ms); 107 ms del hilo de vuelta, 40 de ellos esperando sus
propias copias.

## Reglas para cada etapa

1. Una etapa por commit, con interruptor para apagarla, sobre `amd-optimized`.
2. Antes: `cargo test` de formats, shader, gpu y hle. Los tests de `watch` fallan a
   veces desde antes (estado compartido): se repiten solos y en serie.
3. Medir con el mismo ejecutable, intercalando con/sin el interruptor: una corrida
   fría que no cuenta y dos pares. Se acepta si la mediana baja 5 ms o más en los
   dos pares. La variación normal entre corridas es de 15-30 ms.
4. En cada corrida: 0 caídas de la placa, 0 `request failed`, 0 cierres, y captura
   contra U (traje, malla, paredes, objetos, ojos, buzo en la cinemática).
5. Si falla algo, se apaga el interruptor y se vuelve al commit anterior
   (`prueba-u`, `prueba-ac`). Publicar solo con permiso.

## Etapas

### 1. Lo ya traído (rama `lab-s`, en medición como AE)
- 256 listas en vuelo, cola de copia aparte, bloqueo por rango, descompresión de
  texturas (#603, #993), prueba de límites de profundidad, texturas antes de la
  composición, una copia por textura.
- AE0-AE2 lo miden todo junto contra AD. Si mejora o empata, se pasa a
  `amd-optimized` con cherry-pick (5 commits, sin conflictos con lo actual). Si
  empeora, se separa con `VICTORY_LATE_WAIT_LISTS=128`,
  `VICTORY_RELAY_COPY_QUEUE=0`, `VICTORY_MAPPING_RANGES=0`,
  `VICTORY_EARLY_TEXTURES_MS=0`, `VICTORY_BAKE_TEXTURES_MS=0`.
- Riesgo: la cola de copia lee memoria que otro trabajo puede estar escribiendo;
  mirar parpadeos o datos viejos.
- Además: dejar por defecto los interruptores que ya ganaron
  (`VICTORY_INDIRECT_WINDOW`, `VICTORY_WIDE_REFRESH`, `VICTORY_WIDE_MERGE`) y un
  lanzador `play-amd.ps1` sin DRED.
- Esfuerzo: bajo. Esperado: 10-25 ms quieta; más al moverse.

### 2. Copias sin pasar por el núcleo (hecha, en medición como AF)
- `usercopy.rs`: una rutina propia copia en el lugar; si una página no está, un
  manejador la manda a su salida de error y se sigue por el camino del núcleo.
  Sirve para cualquier tamaño, en los dos sentidos. `VICTORY_USER_COPY=0` la apaga.
- AF1/AF3 con, AF2/AF4 sin.

### 2b. Memoria compartida, parcial (en medición como AG)
- `VICTORY_INPLACE=1`: la memoria privada del juego se abre como memoria de la placa
  y deja de copiarse. Ya estaba en la base, apagado. El nivel 2 no va con los envíos
  adelantados. Además bajaría la memoria comprometida.
- Riesgo: el juego no puede liberar ni proteger esa memoria mientras está abierta.

### 2c. Barrido de texturas ociosas por cola (idea de xcl, PR 490 de allá)
- `resource.rs` `let_go_idle` recorre las ~67.000 texturas en cada barrido y le
  cuesta al hilo que envía ~3 % (perfil P0). Con una cola de "quedó libre a tal
  hora", ordenada, se miran solo las vencidas.
- Esfuerzo: bajo. Esperado: 3-5 ms.

### 3. Preguntar menos qué cambió, y copiar menos
- 9,0 GB revisados por cuadro para encontrar 0,18 GB. Un bloque de 174 MB se revisa
  ~14 veces por cuadro.
- Cómo:
  a. **Granularidad de 4 KB (idea de xcl).** Windows informa escrituras por páginas
     de 4 KB, pero nuestro diario las redondea a 64 KB y copia las 64. Primero medir:
     cuántos de los ~190 MB copiados por cuadro cambiaron de verdad, a 4 KB y por tipo
     de memoria. Si es poco, el diario pasa a 4 KB para la memoria vigilada por Windows.
  b. **Huella por página enviada (idea de xcl).** Si una página se reescribió con lo
     mismo, no se vuelve a mandar a la placa.
  c. Memoria vigilada por protección: un sello por región que sube cada vez que se
     abre un bloque o cambia un mapeo. Un rango sin sellos nuevos se contesta sin
     preguntar. Los conjuntos ordenados (`clean`, `protected`) pasan a mapas de bits.
  d. Una sola pregunta por región y por envío, y ninguna `VirtualQuery` en el camino
     caliente: xcl midió 75-80 ms por llamada en una reserva de 4 GB, con el espacio
     de direcciones bloqueado para todos.
- Archivos: `watch.rs`, `shadow.rs` (`update_free`, `merge_under_writes`).
- Riesgo: un aviso perdido deja datos viejos. Modo de control: un interruptor que
  pregunta igual y cuenta diferencias; tiene que dar 0 antes de activarlo.
- Esfuerzo: medio. Esperado: 15-30 ms, o más si (a) da.

### 4. Sombreadores que marcan lo que escriben (VIctory `cd94d35` + `940d32f`)
- Qué saca: los pases que recalculan y comparan ~2.000 pedazos por cuadro en la
  placa, traer ~30-70 MB y compararlos en la CPU.
- Probado en seco: choca en 17 archivos y 5 de prueba no existen en nuestra base.
  No se trae entero; se porta en tres pasos, cada uno compilable:
  1. Traductor (`victory-shader`): opción `view_marks`, mapa de bits por página de
     64 KiB. Apagado por defecto; validar los ~25.000 sombreadores con el escaneo.
  2. Placa: tabla por casilla y mapa de bits (`descriptor/host.rs`, `draw.rs`,
     `pipeline/host.rs`, `shader_cache.rs`). Primero solo contar: comparar las marcas
     con lo que encuentra la comparación actual. Las marcas tienen que cubrirlo todo.
  3. Vuelta: en `Host::card_before` y `relay_card_back`, usar las marcas en lugar de
     los hashes. `VICTORY_VIEW_MARKS=0` vuelve a lo de hoy.
- También resuelve de raíz el bloque de 174 MB de los 7 sombreadores (hoy se
  aproxima aprendiendo dónde escriben).
- Riesgo: alto; retraduce todo (primera corrida lenta) y agrega código a cada
  escritura. Rama aparte `lab-marcas` desde `amd-optimized`.
- Esfuerzo: alto. Esperado: 20-40 ms del hilo de vuelta y algo de GPU. En el cuadro
  solo se nota si ese hilo es el límite, por eso va después de 2 y 3.

### 5. FSR de AMD (klbf-lab `ffx.rs`)
- Solo si después de 1-4 la GPU pasa a ser el límite (hoy 35 de 167 ms). Archivos
  nuevos, pero depende de la biblioteca FidelityFX que se carga de una carpeta.
- Esfuerzo: medio. Esperado hoy: casi nada.

### 6. Un solo espejo de la memoria del juego (idea de xcl, de fondo)
- xcl no tiene miles de copias: refleja las direcciones del juego en pocas ventanas de
  4 GB en la VRAM (recursos en mosaicos), pregunta qué cambió una vez por envío para
  todo, y sube solo las páginas tocadas con un anillo de 64 MB que no espera.
  Nosotros tenemos ~4.500 copias y 829 ventanas que se superponen.
- Es rehacer `shadow.rs`, los sombreadores que leen por dirección y la vuelta. Solo
  si después de 1-4 el límite sigue en las copias. Esfuerzo: muy alto.
- **Solo ideas:** xcl es GPL-2.0 únicamente y nosotros GPL-3.0 o posterior. No se
  copia código.

## Orden y puntos de decisión

1 → 2 → 2b → perfilar → 2c → 3 (a primero) → perfilar → 4 → 5 → 6.

- Después de 1: si AE no mejora, decidir por pares qué piezas se quedan.
- Después de 3: con un perfil nuevo. Si el límite sigue en el hilo que envía, seguir
  ahí; si pasó al de vuelta, hacer 4.
- Meta razonable de 1 a 3, si las ganancias se suman: 110-130 ms (8-9 fps) quieta.
  Es una estimación, no una promesa.

## Pendiente fuera de este plan
Validación completa de sombreadores (faltó desde R), ropa de la cinemática que va y
viene, restos de planos anteriores abajo, luz rebotada y ray tracing.
