"use strict";

const TOKEN_KEY = "membrie-pairing-token";
let token = localStorage.getItem(TOKEN_KEY) || "";
let statusState = null;
let toastTimer = null;
let selectedAttachment = null;
let attachmentPreviewUrl = "";
let usageStarted = false;

const $ = (selector) => document.querySelector(selector);
const pairScreen = $("#pair-screen");
const statusDot = $("#status-dot");
const statusText = $("#status-text");

async function api(path, options = {}) {
  const headers = new Headers(options.headers || {});
  headers.set("X-Membrie-Token", token);
  if (options.body) headers.set("Content-Type", "application/json");
  const response = await fetch(path, {...options, headers, cache: "no-store"});
  const data = await response.json().catch(() => ({message: "Membrie returned an unreadable response"}));
  if (response.status === 401) {
    token = "";
    localStorage.removeItem(TOKEN_KEY);
    showPairing();
  }
  if (!response.ok) throw new Error(data.message || "The request could not be completed");
  return data;
}

async function exchangePairingCode(code) {
  const response = await fetch("/api/pair", {
    method: "POST",
    headers: {"Content-Type": "application/json"},
    body: JSON.stringify({code}),
    cache: "no-store",
  });
  const data = await response.json().catch(() => ({message: "Membrie returned an unreadable response"}));
  if (!response.ok) throw new Error(data.message || "This device could not be paired");
  if (!/^[0-9a-f]{64}$/.test(data.token || "")) throw new Error("Membrie returned an invalid pairing response");
  return data.token;
}

async function uploadAttachment(file) {
  const response = await fetch("/api/upload", {
    method: "POST",
    headers: {
      "X-Membrie-Token": token,
      "X-Membrie-Filename": encodeURIComponent(file.name || "Pasted image.png"),
      "Content-Type": file.type || "application/octet-stream",
    },
    body: file,
    cache: "no-store",
  });
  const data = await response.json().catch(() => ({message: "Membrie returned an unreadable response"}));
  if (response.status === 401) {
    token = "";
    localStorage.removeItem(TOKEN_KEY);
    showPairing();
  }
  if (!response.ok) throw new Error(data.message || "The attachment could not be uploaded");
  return data.upload_id;
}

function showPairing() {
  pairScreen.hidden = false;
  statusDot.className = "status-dot";
  statusText.textContent = "Pair device";
  $("#pair-token").focus();
}

function hidePairing() {
  pairScreen.hidden = true;
  $("#pair-token").value = "";
}

function setResult(element, message, success = true) {
  element.textContent = message;
  element.className = `result ${success ? "success" : "error"}`;
}

function toast(message) {
  const element = $("#toast");
  element.textContent = message;
  element.classList.add("show");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => element.classList.remove("show"), 2300);
}

async function refreshStatus() {
  statusState = await api("/api/status");
  statusDot.className = `status-dot connected${statusState.locked ? " locked" : ""}`;
  statusText.textContent = statusState.locked ? "PC locked" : "Connected";
  $("#status-detail").textContent = statusState.locked
    ? statusState.allow_while_locked
      ? "The PC is locked, and this paired device is allowed to use recall."
      : "The PC is locked. New notes are accepted, while recall and clipboard access remain private."
    : `${statusState.remembrie_count} Remembries are available on your PC.`;
  renderUsage(statusState.usage);
  return statusState;
}

function renderUsage(usage) {
  if (!usage) return;
  $("#usage-day").textContent = formatUsage(usage.last_24_hours_active_ms, usage.last_24_hours_opens);
  $("#usage-week").textContent = formatUsage(usage.last_7_days_active_ms, usage.last_7_days_opens);
  $("#usage-month").textContent = formatUsage(usage.last_30_days_active_ms, usage.last_30_days_opens);
}

function formatUsage(milliseconds, opens) {
  const minutes = Math.round((milliseconds || 0) / 60000);
  return `${minutes} min · ${opens || 0} open${opens === 1 ? "" : "s"}`;
}

async function recordUsage(activeMs, opened = false) {
  if (!token) return;
  try {
    const usage = await api("/api/usage", {
      method: "POST",
      body: JSON.stringify({active_ms: activeMs, opened}),
    });
    renderUsage(usage);
  } catch (_error) {
    // Usage measurement never interrupts capture or recall.
  }
}

function startUsageTracking() {
  if (usageStarted) return;
  usageStarted = true;
  recordUsage(0, true);
  setInterval(() => {
    if (document.visibilityState === "visible" && token) recordUsage(30000, false);
  }, 30000);
}

$("#pair-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  const result = $("#pair-result");
  const candidate = $("#pair-token").value.trim().toLowerCase();
  if (!/^\d{8}$/.test(candidate) && !/^[0-9a-f]{64}$/.test(candidate)) {
    setResult(result, "Enter the temporary 8-digit code shown on your PC.", false);
    return;
  }
  try {
    token = /^\d{8}$/.test(candidate) ? await exchangePairingCode(candidate) : candidate;
    await refreshStatus();
    localStorage.setItem(TOKEN_KEY, token);
    hidePairing();
    setResult(result, "");
    await refreshRecall();
    startUsageTracking();
  } catch (error) {
    token = "";
    setResult(result, error.message, false);
  }
});

document.querySelectorAll(".nav-item").forEach((button) => {
  button.addEventListener("click", async () => {
    const name = button.dataset.view;
    document.querySelectorAll(".nav-item").forEach((item) => item.classList.toggle("active", item === button));
    document.querySelectorAll(".view").forEach((view) => view.classList.toggle("active", view.id === `view-${name}`));
    if (name === "recall") await refreshRecall();
  });
});

$("#note-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  const button = event.submitter;
  const result = $("#note-result");
  button.disabled = true;
  const title = $("#note-title").value;
  const body = $("#note-body").value;
  if (!selectedAttachment && !title.trim() && !body.trim()) {
    setResult(result, "Write something or attach an image first.", false);
    button.disabled = false;
    return;
  }
  button.textContent = selectedAttachment ? "Sending privately…" : "Remembering…";
  try {
    let response;
    if (selectedAttachment) {
      const uploadId = await uploadAttachment(selectedAttachment);
      response = await api("/api/attachment", {
        method: "POST",
        body: JSON.stringify({upload_id: uploadId, title, body}),
      });
    } else {
      response = await api("/api/note", {
        method: "POST",
        body: JSON.stringify({title, body}),
      });
    }
    $("#note-title").value = "";
    $("#note-body").value = "";
    $("#note-attachment").value = "";
    setSelectedAttachment(null);
    setResult(result, response.message);
    toast("Saved on your PC");
  } catch (error) {
    setResult(result, error.message, false);
  } finally {
    button.disabled = false;
    button.textContent = "Remember on my PC";
  }
});

function setSelectedAttachment(file) {
  selectedAttachment = file;
  if (attachmentPreviewUrl) URL.revokeObjectURL(attachmentPreviewUrl);
  attachmentPreviewUrl = "";
  const preview = $("#attachment-preview");
  preview.replaceChildren();
  if (!file) {
    preview.hidden = true;
    return;
  }
  if (!file.type.startsWith("image/")) {
    setResult($("#note-result"), "This first attachment pass accepts images.", false);
    selectedAttachment = null;
    preview.hidden = true;
    return;
  }
  if (file.size > 100 * 1024 * 1024) {
    setResult($("#note-result"), "That image is larger than 100 MiB.", false);
    selectedAttachment = null;
    preview.hidden = true;
    return;
  }
  attachmentPreviewUrl = URL.createObjectURL(file);
  const image = document.createElement("img");
  image.src = attachmentPreviewUrl;
  image.alt = "Selected image preview";
  preview.append(image, textElement("p", `${file.name || "Pasted image"} · ${formatBytes(file.size)}`));
  preview.hidden = false;
  setResult($("#note-result"), "Image ready to remember.");
}

$("#note-attachment").addEventListener("change", (event) => {
  setSelectedAttachment(event.currentTarget.files[0] || null);
});

$("#note-form").addEventListener("paste", (event) => {
  const image = Array.from(event.clipboardData?.files || []).find((file) => file.type.startsWith("image/"));
  if (image) {
    event.preventDefault();
    setSelectedAttachment(image);
  }
});

$("#note-form").addEventListener("dragover", (event) => event.preventDefault());
$("#note-form").addEventListener("drop", (event) => {
  const image = Array.from(event.dataTransfer?.files || []).find((file) => file.type.startsWith("image/"));
  if (image) {
    event.preventDefault();
    setSelectedAttachment(image);
  }
});

$("#send-clipboard").addEventListener("click", async (event) => {
  const button = event.currentTarget;
  const result = $("#send-clipboard-result");
  button.disabled = true;
  try {
    const response = await api("/api/clipboard", {
      method: "POST",
      body: JSON.stringify({text: $("#phone-clipboard").value}),
    });
    setResult(result, response.message);
    toast("PC clipboard updated");
  } catch (error) {
    setResult(result, error.message, false);
  } finally {
    button.disabled = false;
  }
});

$("#read-clipboard").addEventListener("click", async (event) => {
  const button = event.currentTarget;
  const result = $("#read-clipboard-result");
  button.disabled = true;
  try {
    const response = await api("/api/clipboard");
    $("#pc-clipboard").value = response.text;
    $("#copy-fetched").disabled = false;
    setResult(result, "Fetched explicitly from the PC.");
  } catch (error) {
    setResult(result, error.message, false);
  } finally {
    button.disabled = false;
  }
});

$("#copy-fetched").addEventListener("click", async () => {
  try {
    await navigator.clipboard.writeText($("#pc-clipboard").value);
    toast("Copied on this device");
  } catch (_error) {
    $("#pc-clipboard").select();
    toast("Select and copy the text manually");
  }
});

$("#brie-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  const button = event.submitter;
  const container = $("#brie-answer");
  button.disabled = true;
  button.textContent = "Brie is thinking locally…";
  container.className = "card conversation empty";
  container.replaceChildren(textElement("p", "Checking your Remembries on the PC…"));
  try {
    const response = await api("/api/brie", {
      method: "POST",
      body: JSON.stringify({question: $("#brie-question").value}),
    });
    container.className = "card conversation";
    const answer = textElement("p", response.answer, "answer");
    const model = textElement("p", `Answered locally by ${response.model}`, "model-label");
    const citations = document.createElement("div");
    citations.className = "citations";
    response.citations.forEach((citation) => {
      const card = document.createElement("div");
      card.className = "citation";
      card.append(
        textElement("strong", `[${citation.number}] ${citation.title}`),
        textElement("p", `${citation.kind} · ${citation.source} · ${formatDate(citation.occurred_at_ms)}`),
        textElement("p", citation.excerpt)
      );
      citations.append(card);
    });
    container.replaceChildren(answer, model, citations);
  } catch (error) {
    container.className = "card conversation empty";
    container.replaceChildren(textElement("p", error.message));
  } finally {
    button.disabled = false;
    button.textContent = "Ask Brie";
  }
});

async function refreshRecall() {
  if (!token) return;
  try {
    const response = await api("/api/recall");
    renderMemoryList($("#recent-list"), response.recent, "No recent desktop context is available yet.");
    renderMemoryList($("#upcoming-list"), response.upcoming, "No upcoming calendar events are remembered.");
  } catch (error) {
    renderMemoryList($("#recent-list"), [], error.message);
    $("#upcoming-list").replaceChildren();
  }
}

function renderMemoryList(container, records, emptyMessage) {
  container.replaceChildren();
  if (!records.length) {
    container.append(textElement("div", emptyMessage, "empty-list"));
    return;
  }
  records.forEach((record) => {
    const card = document.createElement("article");
    card.className = "memory-card";
    const meta = document.createElement("div");
    meta.className = "meta";
    meta.append(textElement("span", record.kind), textElement("time", formatDate(record.occurred_at_ms)));
    card.append(meta, textElement("h4", record.title));
    if (record.attachment && record.attachment.mime_type.startsWith("image/")) {
      const image = document.createElement("img");
      image.alt = record.attachment.original_name;
      image.loading = "lazy";
      card.append(image);
      loadAttachmentPreview(record, image);
    }
    if (record.preview) card.append(textElement("p", record.preview));
    card.append(textElement("p", record.source, "model-label"));
    container.append(card);
  });
}

async function loadAttachmentPreview(record, image) {
  try {
    const response = await fetch(`/api/attachment/${encodeURIComponent(record.id)}/${encodeURIComponent(record.attachment.content_id)}`, {
      headers: {"X-Membrie-Token": token},
      cache: "no-store",
    });
    if (!response.ok) throw new Error("preview unavailable");
    const blob = await response.blob();
    const url = URL.createObjectURL(blob);
    image.addEventListener("load", () => URL.revokeObjectURL(url), {once: true});
    image.src = url;
  } catch (_error) {
    image.remove();
  }
}

function formatBytes(bytes) {
  if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`;
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(1)} KiB`;
  return `${bytes} bytes`;
}

function textElement(tag, text, className = "") {
  const element = document.createElement(tag);
  element.textContent = text;
  if (className) element.className = className;
  return element;
}

function formatDate(timestamp) {
  return new Intl.DateTimeFormat(undefined, {month: "short", day: "numeric", hour: "numeric", minute: "2-digit"}).format(new Date(timestamp));
}

$("#refresh-recall").addEventListener("click", refreshRecall);
$("#status-button").addEventListener("click", () => { $("#status-sheet").hidden = false; });
$("#close-status").addEventListener("click", () => { $("#status-sheet").hidden = true; });
$("#forget-device").addEventListener("click", () => {
  token = "";
  localStorage.removeItem(TOKEN_KEY);
  $("#status-sheet").hidden = true;
  showPairing();
});

(async () => {
  const fragment = new URLSearchParams(window.location.hash.slice(1));
  const pairingCode = fragment.get("pair-code") || "";
  if (window.location.hash) history.replaceState(null, "", `${window.location.pathname}${window.location.search}`);
  if (/^\d{8}$/.test(pairingCode)) {
    try {
      token = await exchangePairingCode(pairingCode);
      localStorage.setItem(TOKEN_KEY, token);
      hidePairing();
      await refreshStatus();
      await refreshRecall();
      startUsageTracking();
      toast("This device is paired");
      return;
    } catch (error) {
      token = "";
      showPairing();
      setResult($("#pair-result"), error.message, false);
      return;
    }
  }
  if (!token) {
    showPairing();
    return;
  }
  try {
    await refreshStatus();
    await refreshRecall();
    startUsageTracking();
  } catch (error) {
    showPairing();
    setResult($("#pair-result"), error.message, false);
  }
})();
