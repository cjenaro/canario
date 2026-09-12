// Spanish message catalog (canario-tts) — the first non-English locale.
//
// Typed `Partial<EnglishCatalog>` per the groundwork pattern: missing
// keys fall back to English via the `{ ...en, ...es }` merge in
// index.ts, so a key added to en.ts doesn't block a release on
// translation. This file is nonetheless COMPLETE at the time of
// landing; the fallback is a safety net, not a TODO list.
//
// Conventions: vos-free neutral Spanish (usted-free informal "tú"
// register, standard across the product's tone), keep emoji/symbols
// verbatim, placeholders ({{ … }}) untranslated, and technical terms
// that stay English in Spanish UIs (Base URL, API key, tokens…) kept
// as the community writes them.

import type { Catalog } from "./en";

export const es: Partial<Catalog> = {
  // ── Shared / cross-section ────────────────────────────────────────
  "common.rec": "GRA",
  "common.loading": "Cargando…",
  "common.notSet": "Sin configurar",
  "common.cancel": "Cancelar",
  "common.copy": "Copiar",
  "common.copyTitle": "Copiar al portapapeles",
  "common.copyButton": "📋 Copiar",
  "common.delete": "Eliminar",
  "common.browse": "Examinar…",
  "common.copiedToClipboard": "Copiado al portapapeles",
  "common.couldNotSaveSetting": "No se pudo guardar la configuración.",
  "common.checking": "Comprobando…",
  "common.copying": "Copiando…",
  "common.etaSeconds": "{{n}}s",
  "common.etaMinutes": "{{m}}m {{s}}s",

  // ── Model section (Settings + Onboarding step 1) ──────────────────
  "model.sectionTitle": "Modelo",
  "model.parakeetV3.name": "Parakeet TDT v3",
  "model.parakeetV3.desc": "Multilingüe · ~640 MB",
  "model.parakeetV2.name": "Parakeet TDT v2",
  "model.parakeetV2.desc": "Solo inglés · ~640 MB",
  "model.custom.name": "Modelo personalizado",
  "model.custom.desc": "Archivos sherpa-onnx locales · sin descarga",
  "model.custom.hint":
    "Apunta Canario a tus propios archivos sherpa-onnx. joiner.int8.onnx debe estar junto al encoder.",
  "model.custom.field.encoder": "encoder",
  "model.custom.field.decoder": "decoder",
  "model.custom.field.tokens": "tokens",
  "model.custom.missingPaths":
    "⚠ Configura la{{plural}} ruta{{plural}} {{fields}} — la grabación fallará hasta tener las tres.",
  "model.custom.filesMissing":
    "⚠ Uno o más archivos del modelo no se encontraron en el disco — revisa las rutas de arriba.",
  "model.custom.ready": "✓ El modelo personalizado está listo",
  "model.download": "Descargar {{name}}",
  "model.downloadHint":
    "El modelo de ASR corre localmente en tu equipo. Requiere descarga antes del primer uso.",
  "model.downloading": "Descargando el modelo… puede tardar unos minutos.",
  "model.stopDownloadTitle": "Detener la descarga — el progreso se conserva y se retoma la próxima vez",
  "model.downloadCancelled": "Descarga cancelada — se reanudará la próxima vez",
  "model.ready": "✓ {{name}} está listo",
  "model.deleteFailed": "No se pudo eliminar el modelo.",
  "model.deleted": "Modelo eliminado",
  "model.downloadStartFailed": "No se pudo iniciar la descarga",
  "model.downloadCouldNotStart": "No se pudo iniciar la descarga del modelo",
  "model.notReady": "El modelo no está listo",
  "model.notReadyHint.download":
    "Descarga un modelo de reconocimiento de voz arriba para empezar a transcribir.",
  "model.notReadyHint.custom":
    "Apunta Canario a tus archivos de modelo locales arriba (encoder, decoder, tokens — más joiner.int8.onnx junto al encoder) para empezar a transcribir.",

  // ── Record section (Settings) ─────────────────────────────────────
  "record.sectionTitle": "Grabación",
  "record.recordButton": "🎤 Grabar",
  "record.stopTitle": "Detener y transcribir",
  "record.cancelTitle": "Cancelar (descarta el audio, sin transcripción)",
  "record.cancelled": "Grabación cancelada",
  "record.clickOrHotkey": "Haz clic o pulsa tu atajo para grabar",
  "record.transcribing": "Transcribiendo…",
  "record.listening": "Escuchando… habla ahora",
  "record.failed": "La grabación falló",
  "record.noMic": "No se detectó micrófono. Revisa tu configuración de audio.",

  // ── Hotkey section (Settings) ─────────────────────────────────────
  "hotkey.sectionTitle": "Atajo",
  "hotkey.hint": "Mantén pulsado para grabar. Suelta para detener y transcribir.",
  "hotkey.doubleTapLock.title": "Doble pulsación para bloquear",
  "hotkey.doubleTapLock.desc": "Pulsa dos veces el atajo para alternar la grabación",
  "hotkey.minHold.title": "Tiempo mínimo de pulsación",
  "hotkey.minHold.desc": "Segundos de pulsación antes de iniciar la grabación",
  "hotkey.doubleTapWindow.title": "Ventana de doble pulsación",
  "hotkey.doubleTapWindow.desc":
    "Milisegundos dentro de los cuales dos pulsaciones cuentan como doble",
  "hotkey.capture.pressKeys": "Pulsa una combinación de teclas…",
  "hotkey.capture.escHint": "(Esc para cancelar)",
  "hotkey.capture.change": "Cambiar",
  "hotkey.notice.title": "El atajo todavía no puede leer tu teclado",
  "hotkey.notice.body":
    "En Linux, Canario escucha el atajo global a través de /dev/input, y tu usuario no está en el grupo «input» — el atajo queda mudo. Grabar sigue funcionando desde el botón de arriba, la bandeja o un disparador externo como",
  "hotkey.notice.copyTitle": "Copiar el comando al portapapeles",
  "hotkey.notice.commandCopied": "Comando copiado al portapapeles",
  "hotkey.notice.copyFailed": "No se pudo copiar — selecciona el texto del comando y cópialo manualmente",
  "hotkey.notice.step1": "Copia el comando y ejecútalo en una terminal.",
  "hotkey.notice.step2":
    "Cierra y vuelve a abrir la sesión — la pertenencia al grupo solo aplica en sesiones nuevas.",
  "hotkey.notice.step3": "Inicia Canario de nuevo.",

  // ── Behavior section (Settings) ───────────────────────────────────
  "behavior.sectionTitle": "Comportamiento",
  "behavior.autoPaste.title": "Auto-pegar la transcripción",
  "behavior.autoPaste.desc": "Pegar automáticamente el resultado en la app enfocada",
  "behavior.soundEffects.title": "Efectos de sonido",
  "behavior.soundEffects.desc": "Reproducir sonidos al iniciar/detener la grabación",
  "behavior.soundVolume.title": "Volumen del sonido",
  "behavior.soundVolume.desc": "Intensidad de los pitidos ({{percent}} %)",
  "behavior.liveCaptions.title": "Subtítulos en vivo",
  "behavior.liveCaptions.desc":
    "Mostrar una vista previa de texto en el overlay durante grabaciones largas",
  "behavior.trayIcon.title": "Mostrar icono en la bandeja",
  "behavior.trayIcon.desc": "Mostrar Canario en la bandeja del sistema",
  "behavior.trayIcon.hidden":
    "Icono de bandeja oculto — relanza Canario para reabrir esta ventana",
  "behavior.autostart.title": "Iniciar al entrar",
  "behavior.autostart.desc": "Lanzar Canario al iniciar sesión",
  "behavior.autostart.enabled": "Canario se iniciará al entrar",
  "behavior.autostart.disabled": "Inicio automático desactivado",
  "behavior.autostart.failed": "No se pudo cambiar el ajuste de inicio automático.",
  "behavior.audioBehavior.title": "Audio durante la grabación",
  "behavior.audioBehavior.desc": "Comportamiento del audio del sistema al grabar",
  "behavior.audioBehavior.doNothing": "No hacer nada",
  "behavior.audioBehavior.mute": "Silenciar el audio del sistema",
  "behavior.audioBehavior.muteNote":
    "Silenciar baja la salida de audio predeterminada vía pactl (PulseAudio/PipeWire) mientras graba y restaura su estado anterior cuando la grabación se detiene o cancela. Si pactl no está disponible (p. ej. macOS, Windows o una instalación mínima de Linux), el audio queda activo y se registra un aviso.",

  // ── Microphone section (Settings) ─────────────────────────────────
  "mic.sectionTitle": "Micrófono",
  "mic.title": "Micrófono de dictado",
  "mic.desc": "De qué dispositivo de entrada graba Canario",
  "mic.note":
    "Cambiar libera el micrófono en caliente y lo reabre en el nuevo dispositivo en el próximo dictado.",
  "mic.systemDefault": "Predeterminado del sistema",
  "mic.notConnected": "{{name}} (no conectado)",
  "mic.saveFailed": "No se pudo guardar la selección de micrófono.",

  // ── Word Remapping section (Settings) ─────────────────────────────
  "remap.sectionTitle": "Reemplazo de palabras",
  "remap.empty.title": "Aún no hay reglas. Añade reglas para corregir errores de reconocimiento.",
  "remap.empty.example": "p. ej. «I llama» → «I'll ama»",
  "remap.hint": "Corrige errores comunes de reconocimiento y elimina muletillas",
  "remap.tab.findReplace": "Buscar → Reemplazar",
  "remap.tab.removeWords": "Quitar palabras",
  "remap.field.find": "Buscar",
  "remap.field.replace": "Reemplazar",
  "remap.field.wordToRemove": "Palabra a quitar",

  // ── Transformation section (Settings) ─────────────────────────────
  "transform.sectionTitle": "Transformación",
  "transform.enable.title": "Transformar transcripciones",
  "transform.enable.desc":
    "Limpia cada transcripción con tu propio LLM antes de pegar (desactivado por defecto)",
  "transform.baseUrl.title": "Base URL",
  "transform.baseUrl.invalid": "Escribe una URL http(s):// completa",
  "transform.baseUrl.placeholder": "https://api.openai.com/v1 — o http://localhost:11434/v1 (Ollama)",
  "transform.baseUrl.hint":
    "Cualquier endpoint compatible con OpenAI, incluidos servidores locales (Ollama, llama.cpp) — incluye la ruta de versión.",
  "transform.model.title": "Modelo",
  "transform.model.placeholder": "gpt-4o-mini · llama3 · qwen2.5:7b …",
  "transform.apiKey.title": "Clave de API",
  "transform.apiKey.placeholderPresent": "Clave guardada — escribe para reemplazar, limpia y desenfoca para quitar",
  "transform.apiKey.placeholderAbsent": "sk-… (innecesaria en endpoints locales)",
  "transform.apiKey.presentNote":
    "✓ Clave guardada — cifrada por Canario, nunca se imprime ni se sincroniza",
  "transform.apiKey.absentNote":
    "Sin clave almacenada — los endpoints locales (Ollama, servidor llama.cpp) no la necesitan",
  "transform.timeout.title": "Tiempo de espera",
  "transform.timeout.desc":
    "Milisegundos a esperar antes de caer en la transcripción cruda",
  "transform.warning.title": "Endpoint remoto",
  "transform.warning.bodyIntro": "Las transcripciones y una instrucción breve de estilo se enviarán a",
  "transform.warning.bodyOutro":
    ". Nada más sale nunca de tu equipo — jamás el audio, jamás el historial. Los endpoints locales (localhost) nunca salen de este dispositivo.",
  "transform.warning.dismiss": "No volver a mostrar",
  "transform.test.button": "Probar conexión",
  "transform.test.running": "Probando…",
  "transform.test.ok": "✓ Conectado — {{ms}} ms",
  "transform.test.titleEnabled": "Enviar una petición mínima de chat-completions",
  "transform.test.titleDisabled": "Escribe primero una Base URL válida",
  "transform.test.failed": "Falló la prueba de conexión",
  "transform.test.unreachable": "No se pudo alcanzar el backend de voz",
  "transform.test.unexpected": "Respuesta inesperada del backend de voz",
  "transform.privacyNote":
    "Tu clave queda en este dispositivo (cifrada en reposo) y solo vive en la memoria del backend de transcripción. Si el proveedor falla o se agota el tiempo, se pega la transcripción cruda sin cambios — el dictado nunca se bloquea.",
  "transform.keySaved": "Clave de API guardada",
  "transform.keyRemoved": "Clave de API eliminada",
  "transform.keySaveFailed": "No se pudo guardar la clave de API — reintenta",
  "transform.fellBack": "La transformación cayó en la transcripción cruda",

  // ── Appearance section (Settings) ─────────────────────────────────
  "appearance.sectionTitle": "Apariencia",
  "appearance.mode.dark": "Oscuro",
  "appearance.mode.light": "Claro",
  "appearance.mode.system": "Sistema",
  "appearance.language.title": "Idioma",
  "appearance.language.desc": "Idioma de la interfaz de Canario",
  "appearance.language.auto": "Automático",
  "appearance.language.en": "English",
  "appearance.language.es": "Español",
  "appearance.accent.title": "Color de acento",
  "appearance.accent.desc": "Usado en botones, resaltados y el brillo de grabación",
  "appearance.accent.defaultTitle": "Predeterminado — el acento incorporado de cada tema",
  "appearance.accent.defaultLabel": "Acento predeterminado",
  "appearance.accent.presetAria": "Acento {{name}}",
  "appearance.accent.preset.canary": "Canario",
  "appearance.accent.preset.ocean": "Océano",
  "appearance.accent.preset.violet": "Violeta",
  "appearance.accent.preset.emerald": "Esmeralda",
  "appearance.accent.preset.amber": "Ámbar",
  "appearance.accent.preset.rose": "Rosa",
  "appearance.accent.custom": "Personalizado",
  "appearance.accent.hexPlaceholder": "#RRGGBB",
  "appearance.accent.customLabel": "Color de acento personalizado (hex)",
  "appearance.accent.apply": "Aplicar",
  "appearance.accent.invalidHex": "Escribe un color hex como #e94560 o #f53",

  // ── Indicator (Appearance area, Settings) ─────────────────────────
  "indicator.title": "Indicador",
  "indicator.desc": "Qué aparece en pantalla mientras dictas",
  "indicator.full.name": "Overlay completo",
  "indicator.full.desc": "Píldora de grabación con cronómetro, subtítulos en vivo y fases de transcripción",
  "indicator.dot.name": "Punto",
  "indicator.dot.desc": "Un punto mínimo pulsante mientras graba — nada más en pantalla",
  "indicator.tray.name": "Solo bandeja",
  "indicator.tray.desc": "Sin indicador en pantalla; el icono de bandeja muestra el estado de grabación",
  "indicator.note":
    "El punto y el overlay completo comparten una posición por monitor — arrastra el overlay completo para ubicar ambos. Cambiar de modo a mitad de grabación aplica al instante; al salir de «Solo bandeja» el indicador reaparece en la próxima grabación.",
  "indicator.saveFailed": "No se pudo guardar el ajuste del indicador.",

  // ── Motion section (Settings) ─────────────────────────────────────
  "motion.sectionTitle": "Animaciones",
  "motion.master.title": "Animaciones",
  "motion.master.desc": "Reproducir las animaciones de la interfaz",
  "motion.reducedMotion":
    "Tu sistema pide movimiento reducido — las animaciones quedan desactivadas mientras ese ajuste del SO esté activo, sin importar los interruptores de aquí.",
  "motion.effect.overlay_slide.name": "Deslizamiento del overlay",
  "motion.effect.overlay_slide.desc": "La isla de grabación se desliza al iniciar la grabación",
  "motion.effect.recording_dot_pulse.name": "Pulso del punto de grabación",
  "motion.effect.recording_dot_pulse.desc": "Punto rojo pulsante mientras graba",
  "motion.effect.toggle_slide.name": "Deslizamiento de interruptores",
  "motion.effect.toggle_slide.desc": "Los interruptores se deslizan y cambian de color",
  "motion.effect.delete_slide.name": "Deslizamiento de eliminación",
  "motion.effect.delete_slide.desc": "Los elementos del historial se deslizan al eliminarse",
  "motion.effect.window_fade.name": "Fundido de ventanas",
  "motion.effect.window_fade.desc": "Las ventanas aparecen con fundido y escala al abrirse",

  // ── About section (Settings) ──────────────────────────────────────
  "about.sectionTitle": "Acerca de",
  "about.version.title": "Versión",
  "about.version.withSidecar": "{{app}} (sidecar {{sidecar}})",
  "about.version.checkButton": "Buscar actualizaciones",
  "about.update.title": "Actualización disponible: v{{version}}",
  "about.update.desc": "Reinicia Canario para instalar la última versión.",
  "about.upToDate": "Canario está actualizado",
  "about.updateCheckFailed": "No se pudieron buscar actualizaciones",
  "about.onboarding.title": "Configuración inicial",
  "about.onboarding.desc": "Repetir el asistente del primer arranque",
  "about.onboarding.rerun": "Repetir",
  "about.diagnostics.title": "Diagnóstico",
  "about.diagnostics.desc": "Copiar info del sistema, configuración y registros recientes",
  "about.diagnostics.copy": "Copiar diagnóstico",
  "about.diagnostics.collectFailed": "No se pudo recopilar el diagnóstico. Revisa que el sidecar esté corriendo.",
  "about.diagnostics.copied": "Diagnóstico copiado al portapapeles",
  "about.diagnostics.copyFailed": "No se pudo copiar el diagnóstico al portapapeles",
  "about.versionMismatch.title": "⚠ Incompatibilidad de versiones",
  "about.versionMismatch.noProtocol":
    "El backend de voz es anterior al versionado de protocolo — los comandos y eventos pueden haber divergido. Reinicia con una build que coincida.",
  "about.versionMismatch.protocol":
    "La app y el backend de voz hablan versiones de protocolo distintas (el backend reporta {{protocol}}). Reinicia con una build que coincida.",
  "about.versionMismatch.staleSidecar":
    "El backend de voz {{sidecar}} no coincide con la app {{app}} — puede haber un backend obsoleto corriendo. Reinicia Canario.",

  // ── History section (Settings) ────────────────────────────────────
  "history.sectionTitle": "Historial",
  "history.lastTitle": "Última transcripción",
  "history.clearAll": "Borrar todo",
  "history.searchPlaceholder": "🔍  Buscar transcripciones…",
  "history.empty.title": "Aún no hay transcripciones",
  "history.empty.hint": "¡Pulsa tu atajo y empieza a hablar!",
  "history.noResults": "Sin resultados para «{{query}}»",
  "history.clearSearch": "Limpiar búsqueda",
  "history.cleared": "Historial borrado",
  "history.deleteFailed": "No se pudo eliminar la entrada.",
  "history.meta": "{{duration}}s · {{timestamp}}",
  "history.transformedBadge": "✨ Transformada",
  "history.transformedTitle": "Transformada — transcripción cruda: {{raw}}",
  "history.time.justNow": "Recién",
  "history.time.minutesAgo": "hace {{n}} min",
  "history.time.todayAt": "Hoy a las {{time}}",
  "history.time.yesterdayAt": "Ayer a las {{time}}",
  "history.time.dateAt": "{{date}} a las {{time}}",

  // ── Overlay window ────────────────────────────────────────────────
  "overlay.transcribing": "Transcribiendo…",
  "overlay.transforming": "Transformando…",
  "overlay.dragTitle": "Arrastra para mover · doble clic para restablecer",

  // ── Onboarding wizard ─────────────────────────────────────────────
  "onboarding.header": "Bienvenido a Canario",
  "onboarding.skip": "Omitir configuración",
  "onboarding.tagline":
    "De voz a texto, instantáneo e invisible. Pulsa un atajo, habla, suelta. Listo.",
  "onboarding.step.downloadModel": "Descargar modelo",
  "onboarding.step.setHotkey": "Configurar atajo",
  "onboarding.step.ready": "Listo",
  "onboarding.step1.title": "Paso 1 de 3: Descargar modelo",
  "onboarding.step1.desc":
    "Canario usa Parakeet TDT — un modelo de reconocimiento de voz puntero que corre por completo en tu dispositivo. Nada de lo que digas sale jamás de tu equipo.",
  "onboarding.step1.continueWithout":
    "Puedes continuar sin el modelo, pero la transcripción no funcionará hasta descargarlo.",
  "onboarding.step1.modelDownloaded": "Modelo descargado — ¡todo listo!",
  "onboarding.micTest.title": "🎤 Prueba de micrófono",
  "onboarding.micTest.start": "Probar micrófono",
  "onboarding.micTest.stop": "Detener",
  "onboarding.micTest.saying": "Di algo…",
  "onboarding.micTest.desc": "Graba {{secs}}s de audio para comprobar tu nivel de micrófono.",
  "onboarding.micTest.noAccess": "No se pudo acceder al micrófono. Revisa tu configuración de audio.",
  "onboarding.dlStats.plain": "{{done}} / {{total}} MB",
  "onboarding.dlStats.speed": "{{done}} / {{total}} MB · {{speed}} MB/s",
  "onboarding.dlStats.etaSuffix": " · ~{{eta}} restantes",
  "onboarding.next": "Siguiente →",
  "onboarding.back": "← Atrás",
  "onboarding.step2.title": "Paso 2 de 3: Configurar atajo",
  "onboarding.step2.desc":
    "Elige una combinación de teclas que inicie y detenga la grabación desde cualquier parte.",
  "onboarding.step2.pressHoldLabel": "Mantener pulsado:",
  "onboarding.step2.pressHoldBody": "mantén la combinación mientras hablas, suelta para transcribir.",
  "onboarding.step2.doubleTapLabel": "Doble pulsación:",
  "onboarding.step2.doubleTapBody":
    "pulsa la combinación para iniciar la grabación, pulsa de nuevo para detener — manos libres para dictados largos.",
  "onboarding.step2.linux": "En Linux el atajo lo gestiona el propio listener de Canario.",
  "onboarding.step2.other": "En esta plataforma el atajo se registra globalmente con el SO.",
  "onboarding.step3.title": "Paso 3 de 3: Listo",
  "onboarding.step3.descIntro": "¡Pruébalo ahora! Haz clic en el campo de abajo, pulsa",
  "onboarding.step3.yourHotkey": "tu atajo",
  "onboarding.step3.descOutro": ", habla y suelta — tus palabras aparecerán aquí mismo.",
  "onboarding.step3.placeholder": "Pulsa tu atajo y di algo…",
  "onboarding.step3.works": "✓ ¡Funciona! Última transcripción: «{{text}}»",
  "onboarding.step3.noModel":
    "Atención: todavía no hay modelo de voz descargado, así que el dictado de práctica no transcribirá. Puedes descargarlo luego desde Configuración → Modelo.",
  "onboarding.done": "Listo — minimizar a la bandeja",
  "onboarding.initFailed": "No se pudo inicializar. Revisa que el sidecar de canario esté corriendo.",

  // ── Renderer-authored error strings (createCanario bridge) ────────
  "errors.sidecarCrashed":
    "El backend de voz terminó inesperadamente (código {{code}}) — reinicia Canario",
  "errors.initFailed.electron":
    "No se pudo inicializar. Revisa que el sidecar canario-electron esté corriendo.",
};
