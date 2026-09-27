"use strict";

const TOKEN_KEY = "membrie-pairing-token";
let token = localStorage.getItem(TOKEN_KEY) || "";
let statusState = null;
let toastTimer = null;
let selectedAttachments = [];
const attachmentPreviewUrls = new Map();
let usageStarted = false;
let pastedItemSequence = 0;

const MAX_ATTACHMENTS = 8;
const MAX_ATTACHMENT_BYTES = 100 * 1024 * 1024;

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
      "X-Membrie-Filename": encodeURIComponent(file.name || clipboardFilename(file.type)),
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
  if (!selectedAttachments.length && !title.trim() && !body.trim()) {
    setResult(result, "Paste, write, or attach something first.", false);
    button.disabled = false;
    return;
  }
  button.textContent = selectedAttachments.length ? "Sending privately…" : "Remembering…";
  const uploadIds = [];
  try {
    let response;
    if (selectedAttachments.length) {
      for (const attachment of selectedAttachments) {
        uploadIds.push(await uploadAttachment(attachment));
      }
      response = await api("/api/attachment", {
        method: "POST",
        body: JSON.stringify({upload_ids: uploadIds, title, body}),
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
    clearAttachments();
    setResult(result, response.message);
    toast("Saved on your PC");
  } catch (error) {
    if (uploadIds.length) cancelUploads(uploadIds);
    setResult(result, error.message, false);
  } finally {
    button.disabled = false;
    button.textContent = "Remember on my PC";
  }
});

async function cancelUploads(uploadIds) {
  try {
    await api("/api/uploads/cancel", {
      method: "POST",
      body: JSON.stringify({upload_ids: uploadIds}),
    });
  } catch (_error) {
    // Private staged uploads also expire locally; capture errors remain the useful message.
  }
}

function clearAttachments() {
  selectedAttachments = [];
  attachmentPreviewUrls.forEach((url) => URL.revokeObjectURL(url));
  attachmentPreviewUrls.clear();
  renderAttachments();
}

function addAttachments(files) {
  let total = selectedAttachments.reduce((sum, file) => sum + file.size, 0);
  const rejected = [];
  for (const file of files) {
    if (!(file instanceof Blob) || file.size === 0) {
      rejected.push("an empty item");
      continue;
    }
    if (selectedAttachments.length >= MAX_ATTACHMENTS) {
      rejected.push(`more than ${MAX_ATTACHMENTS} items`);
      break;
    }
    if (total + file.size > MAX_ATTACHMENT_BYTES) {
      rejected.push("more than 100 MiB total");
      continue;
    }
    const duplicate = selectedAttachments.some((existing) =>
      existing.name === file.name && existing.size === file.size && existing.type === file.type
        && existing.lastModified === file.lastModified
    );
    if (duplicate) continue;
    selectedAttachments.push(file);
    total += file.size;
  }
  renderAttachments();
  if (rejected.length) {
    setResult($("#note-result"), `Some clipboard items were not added: ${rejected.join(", ")}.`, false);
  } else if (files.length) {
    setResult(
      $("#note-result"),
      `${selectedAttachments.length} attachment${selectedAttachments.length === 1 ? "" : "s"} ready. Review, then remember.`
    );
  }
}

function removeAttachment(index) {
  const [removed] = selectedAttachments.splice(index, 1);
  const url = attachmentPreviewUrls.get(removed);
  if (url) URL.revokeObjectURL(url);
  attachmentPreviewUrls.delete(removed);
  renderAttachments();
}

function renderAttachments() {
  const preview = $("#attachment-preview");
  preview.replaceChildren();
  if (!selectedAttachments.length) {
    preview.hidden = true;
    return;
  }
  selectedAttachments.forEach((file, index) => {
    const item = document.createElement("div");
    item.className = "attachment-item";
    let visual;
    if (file.type.startsWith("image/")) {
      visual = document.createElement("img");
      visual.className = "attachment-thumb";
      visual.alt = "";
      let url = attachmentPreviewUrls.get(file);
      if (!url) {
        url = URL.createObjectURL(file);
        attachmentPreviewUrls.set(file, url);
      }
      visual.src = url;
    } else {
      visual = textElement("div", fileKind(file), "attachment-kind");
    }
    const copy = document.createElement("div");
    copy.className = "attachment-copy";
    copy.append(
      textElement("strong", file.name || clipboardFilename(file.type)),
      textElement("span", `${file.type || "unknown format"} · ${formatBytes(file.size)}`)
    );
    const remove = textElement("button", "×", "remove-attachment");
    remove.type = "button";
    remove.setAttribute("aria-label", `Remove ${file.name || "attachment"}`);
    remove.addEventListener("click", () => removeAttachment(index));
    item.append(visual, copy, remove);
    preview.append(item);
  });
  preview.hidden = false;
}

$("#note-attachment").addEventListener("change", (event) => {
  addAttachments(Array.from(event.currentTarget.files || []));
  event.currentTarget.value = "";
});

function appendNoteText(text) {
  const addition = String(text || "").trim();
  if (!addition) return false;
  const body = $("#note-body");
  const combined = [body.value.trimEnd(), addition].filter(Boolean).join("\n\n");
  if (combined.length > 20000) {
    setResult($("#note-result"), "That pasted text is longer than the 20,000-character note limit.", false);
    return false;
  }
  body.value = combined;
  return true;
}

function textFromHtml(html) {
  const documentFragment = new DOMParser().parseFromString(html, "text/html");
  documentFragment.querySelectorAll("script, style, template").forEach((node) => node.remove());
  return documentFragment.body?.textContent || "";
}

function filesFromTransfer(transfer) {
  const files = Array.from(transfer?.files || []);
  for (const item of Array.from(transfer?.items || [])) {
    if (item.kind !== "file") continue;
    const file = item.getAsFile();
    if (file && !files.some((candidate) =>
      candidate.name === file.name && candidate.size === file.size && candidate.type === file.type
        && candidate.lastModified === file.lastModified
    )) files.push(file);
  }
  return files;
}

function ingestTransfer(transfer) {
  const files = filesFromTransfer(transfer);
  if (files.length) addAttachments(files);
  let text = transfer?.getData("text/uri-list") || transfer?.getData("text/plain") || "";
  if (!text) text = textFromHtml(transfer?.getData("text/html") || "");
  if (/^file:\/\//i.test(text.trim())) text = "";
  const addedText = appendNoteText(text);
  if (addedText) setResult($("#note-result"), "Pasted text is ready. Review, then remember.");
  return files.length > 0 || addedText;
}

function clipboardFilename(type, numbered = false) {
  const extension = {
    "image/png": "png", "image/jpeg": "jpg", "image/gif": "gif", "image/webp": "webp",
    "image/heic": "heic", "application/pdf": "pdf", "audio/mpeg": "mp3",
    "audio/mp4": "m4a", "audio/x-m4a": "m4a", "video/mp4": "mp4", "video/quicktime": "mov",
  }[type] || "bin";
  const suffix = numbered ? ` ${++pastedItemSequence}` : "";
  return `Pasted item${suffix}.${extension}`;
}

function fileKind(file) {
  const subtype = (file.type || "file").split("/").pop() || "file";
  return subtype.slice(0, 6);
}

async function pasteFromClipboard() {
  if (!navigator.clipboard?.read) throw new Error("Use the long-press Paste area below on this iPhone.");
  const clipboardItems = await navigator.clipboard.read();
  const files = [];
  const texts = [];
  for (const item of clipboardItems) {
    const binaryType = item.types.find((type) => !["text/plain", "text/html", "text/uri-list"].includes(type));
    if (binaryType) {
      const blob = await item.getType(binaryType);
      files.push(new File([blob], clipboardFilename(binaryType, true), {type: binaryType, lastModified: Date.now()}));
      continue;
    }
    const textType = ["text/uri-list", "text/plain", "text/html"].find((type) => item.types.includes(type));
    if (!textType) continue;
    const text = await (await item.getType(textType)).text();
    texts.push(textType === "text/html" ? textFromHtml(text) : text);
  }
  if (files.length) addAttachments(files);
  const addedText = appendNoteText(texts.join("\n\n"));
  if (!files.length && !addedText) throw new Error("iOS did not expose usable clipboard content.");
  if (addedText) setResult($("#note-result"), "Pasted content is ready. Review, then remember.");
}

$("#paste-from-device").addEventListener("click", async (event) => {
  const button = event.currentTarget;
  button.disabled = true;
  try {
    await pasteFromClipboard();
  } catch (error) {
    $("#paste-target").focus();
    setResult($("#note-result"), error.message, false);
  } finally {
    button.disabled = false;
  }
});

$("#paste-target").addEventListener("paste", (event) => {
  event.preventDefault();
  ingestTransfer(event.clipboardData);
  event.currentTarget.replaceChildren();
});

$("#paste-target").addEventListener("input", (event) => {
  const text = event.currentTarget.textContent;
  if (appendNoteText(text)) setResult($("#note-result"), "Pasted text is ready. Review, then remember.");
  event.currentTarget.replaceChildren();
});

$("#note-form").addEventListener("paste", (event) => {
  if (event.target === $("#paste-target")) return;
  const files = filesFromTransfer(event.clipboardData);
  if (files.length) {
    event.preventDefault();
    addAttachments(files);
  }
});

$("#note-form").addEventListener("dragover", (event) => event.preventDefault());
$("#note-form").addEventListener("drop", (event) => {
  const files = filesFromTransfer(event.dataTransfer);
  if (files.length) {
    event.preventDefault();
    addAttachments(files);
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
    (record.attachments || []).forEach((attachment) => {
      if (["image/png", "image/jpeg", "image/gif", "image/webp"].includes(attachment.mime_type)) {
        const image = document.createElement("img");
        image.alt = attachment.original_name;
        image.loading = "lazy";
        card.append(image);
        loadAttachmentPreview(record.id, attachment, image);
      } else {
        card.append(textElement(
          "div",
          `${attachment.original_name} · ${attachment.analysis_state === "unsupported" ? "original retained" : attachment.analysis_state}`,
          "memory-attachment"
        ));
      }
    });
    if (record.preview) card.append(textElement("p", record.preview));
    card.append(textElement("p", record.source, "model-label"));
    container.append(card);
  });
}

async function loadAttachmentPreview(remembrieId, attachment, image) {
  try {
    const response = await fetch(`/api/attachment/${encodeURIComponent(remembrieId)}/${encodeURIComponent(attachment.content_id)}`, {
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
