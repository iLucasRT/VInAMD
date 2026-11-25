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

### 2. Copias chicas sin pasar por el núcleo
- Hoy cada una de las ~13.500 copias cuesta ~2,5 µs en `ReadProcessMemory`
  (`host.rs` `copy_mapped`, `read`). Copiar 500 bytes directo cuesta menos de 0,1 µs.
- Cómo: con el bloqueo por rango de la etapa 1 (`title_memory_reading(at, len)`)
  la memoria no puede desmapearse mientras se lee. Se guarda, por bloque de 64 KiB,
  que está mapeado; `title_memory_remapped` lo invalida. Si el bloque es conocido:
  `memcpy`; si no, el camino actual. Solo para origen del juego y hasta 64 KiB.
- Archivos: `host.rs`, `watch.rs`. Test: remapear mientras otro hilo copia.
- Riesgo: un fallo de acceso si una liberación no pasa por nuestros avisos. Por eso
  solo bloques de vistas mapeadas por la capa, y el resto por el núcleo.
- Esfuerzo: medio. Esperado: 15-25 ms del hilo que envía.

### 3. Preguntar menos qué cambió
- 9,0 GB revisados por cuadro para encontrar 0,18 GB. Un bloque de 174 MB se revisa
  ~14 veces por cuadro.
- Cómo: (a) preguntar una vez por envío por región y responder desde el diario a
  todos los refrescos del envío; (b) para memoria vigilada por protección, cambiar
  los conjuntos ordenados (`clean`, `protected`) por mapas de bits por región, así
  "nada cambió" se contesta en microsegundos; (c) no volver a preguntar por un rango
  que nadie escribió desde el envío anterior (contador de fallos de guardia).
- Archivos: `watch.rs`, `shadow.rs` (`update_free`, `merge_under_writes`).
- Riesgo: un aviso perdido deja datos viejos. Modo de control: un interruptor que
  pregunta igual y cuenta diferencias; tiene que dar 0 antes de activarlo.
- Esfuerzo: medio. Esperado: 15-30 ms.

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

## Orden y puntos de decisión

1 → 2 → 3 → perfilar de nuevo → 4 → 5.

- Después de 1: si AE no mejora, decidir por pares qué piezas se quedan.
- Después de 3: con un perfil nuevo. Si el límite sigue en el hilo que envía, seguir
  ahí; si pasó al de vuelta, hacer 4.
- Meta razonable de 1 a 3, si las ganancias se suman: 110-130 ms (8-9 fps) quieta.
  Es una estimación, no una promesa.

## Pendiente fuera de este plan
Validación completa de sombreadores (faltó desde R), ropa de la cinemática que va y
viene, restos de planos anteriores abajo, luz rebotada y ray tracing.
