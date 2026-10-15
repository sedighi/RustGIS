// The rustgis.Map widget: frames the RustGIS web app (`?embed`) and passes the project between it
// and Python. Python's project (`_project`) is sent to the app when it starts and whenever Python
// changes it; the app's project, after a change there, goes to Python as a message. The app's own
// text is never sent back to it. The protocol is described in apps/rustgis-web/src/embed.rs.

function render({ model, el }) {
  const src = model.get("_src");
  const origin = new URL(src, window.location.href).origin;
  const iframe = document.createElement("iframe");
  iframe.src = src;
  iframe.style.width = "100%";
  iframe.style.height = model.get("height") || "800px";
  iframe.style.border = "0";
  iframe.style.display = "block";
  iframe.allow = "fullscreen; clipboard-read; clipboard-write";
  iframe.allowFullscreen = true;

  let ready = false;
  // The project the app last sent: Python hands it back unchanged, and it isn't echoed.
  let fromApp = null;
  // Commands asked for before the app started.
  let waiting = [];
  let next = 1;

  const post = (msg) => {
    if (iframe.contentWindow) iframe.contentWindow.postMessage(msg, origin);
  };
  const sendProject = () => {
    const text = model.get("_project");
    if (ready && text && text !== fromApp) post({ rustgis: "project", text });
  };
  const onMessage = (event) => {
    if (event.source !== iframe.contentWindow || event.origin !== origin) return;
    const msg = event.data;
    if (!msg || typeof msg !== "object") return;
    if (msg.rustgis === "ready") {
      // Started, or started again (reloaded): it has nothing until it is sent the project.
      ready = true;
      fromApp = null;
      sendProject();
      for (const run of waiting) post(run);
      waiting = [];
    } else if (msg.rustgis === "project" && typeof msg.text === "string") {
      fromApp = msg.text;
      model.send({ type: "project", text: msg.text });
    } else if (msg.rustgis === "result") {
      model.send({ type: "result", ok: msg.ok === true, error: msg.error });
    }
  };
  const onCustom = (msg) => {
    if (!msg || msg.type !== "run") return;
    const run = { rustgis: "run", id: next++, command: msg.command, params: msg.params || {} };
    if (ready) post(run);
    else waiting.push(run);
  };
  const onHeight = () => {
    iframe.style.height = model.get("height") || "800px";
  };

  window.addEventListener("message", onMessage);
  model.on("change:_project", sendProject);
  model.on("change:height", onHeight);
  model.on("msg:custom", onCustom);
  el.appendChild(iframe);

  return () => {
    window.removeEventListener("message", onMessage);
    model.off("change:_project", sendProject);
    model.off("change:height", onHeight);
    model.off("msg:custom", onCustom);
  };
}

export default { render };
