"use strict";

const TOKEN_KEY = "membrie-pairing-token";
let token = localStorage.getItem(TOKEN_KEY) || "";
let statusState = null;
let toastTimer = null;

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
  return statusState;
}

$("#pair-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  const result = $("#pair-result");
  const candidate = $("#pair-token").value.trim().toLowerCase();
  if (!/^[0-9a-f]{64}$/.test(candidate)) {
    setResult(result, "The token should contain 64 letters and numbers.", false);
    return;
  }
  token = candidate;
  try {
    await refreshStatus();
    localStorage.setItem(TOKEN_KEY, token);
    hidePairing();
    setResult(result, "");
    await refreshRecall();
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
  button.textContent = "Remembering…";
  try {
    const response = await api("/api/note", {
      method: "POST",
      body: JSON.stringify({title: $("#note-title").value, body: $("#note-body").value}),
    });
    $("#note-title").value = "";
    $("#note-body").value = "";
    setResult(result, response.message);
    toast("Saved on your PC");
  } catch (error) {
    setResult(result, error.message, false);
  } finally {
    button.disabled = false;
    button.textContent = "Remember on my PC";
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
    if (record.preview) card.append(textElement("p", record.preview));
    card.append(textElement("p", record.source, "model-label"));
    container.append(card);
  });
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
  if (!token) {
    showPairing();
    return;
  }
  try {
    await refreshStatus();
    await refreshRecall();
  } catch (error) {
    showPairing();
    setResult($("#pair-result"), error.message, false);
  }
})();
