const invoke = window.__TAURI__?.core?.invoke;
const listen = window.__TAURI__?.event?.listen;

let appState = {
  app_version: "0.1.3",
  profiles: [],
  single_switch_mode: false,
  status: null,
  preview: null,
  port_statuses: [],
  proxy_running: false,
  proxy_port: 80,
};
let selectedProfileId = null;
let editorProfileId = null;
let isDirty = false;
let pendingUpdate = null;
let toastTimer = null;

// DOM Elements
const profileListEl = document.getElementById("profile-list");
const ledgerSummaryEl = document.getElementById("ledger-summary");
const chkSingleMode = document.getElementById("chk-single-mode");
const inputProfileName = document.getElementById("input-profile-name");
const badgeActiveState = document.getElementById("badge-active-state");
const badgeDirtyState = document.getElementById("badge-dirty-state");
const textarea = document.getElementById("hosts-textarea");
const lineNumbers = document.getElementById("line-numbers");
const validationSummary = document.getElementById("validation-summary");
const portMappingSummary = document.getElementById("port-mapping-summary");
const permDot = document.getElementById("perm-dot");
const permText = document.getElementById("perm-text");
const btnGrantPerm = document.getElementById("btn-grant-perm");
const btnDeleteProfile = document.getElementById("btn-delete-profile");
const appVersionEl = document.getElementById("app-version");
const updateBanner = document.getElementById("update-banner");
const updateText = document.getElementById("update-text");
const btnInstallUpdate = document.getElementById("btn-install-update");
const btnCheckUpdate = document.getElementById("btn-check-update");
const toastEl = document.getElementById("toast");

// Dialogs
const dialogNewProfile = document.getElementById("dialog-new-profile");
const formNewProfile = document.getElementById("form-new-profile");
const newProfileNameInput = document.getElementById("new-profile-name");
const dialogConfirm = document.getElementById("dialog-confirm");
const confirmTitle = document.getElementById("confirm-title");
const confirmMessage = document.getElementById("confirm-message");
const btnConfirmCancel = document.getElementById("btn-confirm-cancel");
const btnConfirmOk = document.getElementById("btn-confirm-ok");
const dialogPreview = document.getElementById("dialog-preview");
const mergedPreviewCode = document.getElementById("merged-preview-code");

function showToast(message) {
  if (!toastEl) return;
  toastEl.textContent = message;
  toastEl.classList.remove("hidden");
  if (toastTimer) clearTimeout(toastTimer);
  toastTimer = setTimeout(() => {
    toastEl.classList.add("hidden");
  }, 2600);
}

function askConfirm(title, message, okLabel = "확인") {
  return new Promise((resolve) => {
    confirmTitle.textContent = title;
    confirmMessage.textContent = message;
    btnConfirmOk.textContent = okLabel;

    let settled = false;
    const cleanup = (result) => {
      if (settled) return;
      settled = true;
      btnConfirmCancel.onclick = null;
      btnConfirmOk.onclick = null;
      dialogConfirm.oncancel = null;
      dialogConfirm.onclose = null;
      if (dialogConfirm.open) {
        dialogConfirm.close();
      }
      resolve(result);
    };

    btnConfirmCancel.onclick = () => cleanup(false);
    btnConfirmOk.onclick = () => cleanup(true);
    dialogConfirm.oncancel = () => cleanup(false);
    dialogConfirm.onclose = () => cleanup(false);
    dialogConfirm.showModal();
  });
}

async function loadSnapshot() {
  if (!invoke) return;
  const snapshot = await invoke("get_app_snapshot");
  applySnapshot(snapshot);
}

function applySnapshot(snapshot) {
  appState = snapshot;
  if (snapshot.app_version && appVersionEl) {
    appVersionEl.textContent = `v${snapshot.app_version}`;
  }
  chkSingleMode.checked = snapshot.single_switch_mode;

  if (!selectedProfileId || !snapshot.profiles.some((p) => p.id === selectedProfileId)) {
    selectedProfileId = snapshot.profiles[0]?.id || null;
  }

  renderLedgerSummary();
  renderSidebar();
  renderPermissionStatus();
  renderSelectedProfile();
  renderPortMappings();
}

function renderLedgerSummary() {
  const activeCount = appState.profiles.filter((p) => p.enabled).length;
  const previewText = appState.preview?.hosts_content || "";
  const domainLines = previewText
    .split("\n")
    .filter((l) => l.trim() && !l.trim().startsWith("#")).length;
  const proxyCount = (appState.port_statuses || []).length;
  ledgerSummaryEl.textContent = `활성 ${activeCount}개 · 규칙 ${domainLines}건 · 프록시 ${proxyCount}건`;
}

function renderPermissionStatus() {
  if (!appState.status) return;
  if (appState.status.hosts_writable) {
    permDot.className = "status-dot ok";
    permText.textContent = "즉시 전환(0ms) 활성";
    btnGrantPerm.classList.add("hidden");
  } else {
    permDot.className = "status-dot";
    permText.textContent = "hosts 쓰기 권한 필요";
    btnGrantPerm.classList.remove("hidden");
  }
}

function renderSidebar() {
  profileListEl.innerHTML = "";
  appState.profiles.forEach((profile) => {
    const li = document.createElement("li");
    const classes = ["profile-item"];
    if (profile.id === selectedProfileId) classes.push("selected");
    if (profile.enabled) classes.push("is-enabled");
    li.className = classes.join(" ");
    li.setAttribute("role", "option");
    li.setAttribute("aria-selected", profile.id === selectedProfileId ? "true" : "false");

    li.onclick = async (e) => {
      if (e.target.tagName === "INPUT" || e.target.classList.contains("slider")) return;
      if (isDirty && profile.id !== selectedProfileId) {
        const ok = await askConfirm(
          "저장하지 않은 변경 사항",
          "현재 프로필의 변경 사항이 저장되지 않았습니다. 저장하지 않고 이동하시겠습니까?",
          "저장하지 않고 이동"
        );
        if (!ok) return;
        setDirty(false);
      }
      selectedProfileId = profile.id;
      renderSidebar();
      renderSelectedProfile();
    };

    const left = document.createElement("div");
    left.className = "profile-left";

    const nameSpan = document.createElement("span");
    nameSpan.className = "profile-name";
    nameSpan.textContent = profile.is_system_base
      ? profile.name.replace(/\s*\(Base\)$/i, "")
      : profile.name;
    nameSpan.title = profile.name;
    left.appendChild(nameSpan);

    if (profile.is_system_base) {
      const baseTag = document.createElement("span");
      baseTag.className = "tag-base num";
      baseTag.textContent = "BASE";
      left.appendChild(baseTag);
    }

    const label = document.createElement("label");
    label.className = "switch";
    label.title = profile.enabled ? "프로필 끄기" : "프로필 켜기";
    const checkbox = document.createElement("input");
    checkbox.type = "checkbox";
    checkbox.checked = profile.enabled;
    checkbox.onchange = async () => {
      try {
        const nextSnapshot = await invoke("toggle_profile", {
          id: profile.id,
          enabled: checkbox.checked,
        });
        applySnapshot(nextSnapshot);
        showToast(`${profile.name} ${checkbox.checked ? "적용됨" : "해제됨"}`);
      } catch (err) {
        showToast(`전환 실패: ${err}`);
        checkbox.checked = !checkbox.checked;
      }
    };

    const slider = document.createElement("span");
    slider.className = "slider";
    label.appendChild(checkbox);
    label.appendChild(slider);

    li.appendChild(left);
    li.appendChild(label);
    profileListEl.appendChild(li);
  });
}

function setDirty(dirty) {
  isDirty = dirty;
  if (dirty) {
    badgeDirtyState.classList.remove("hidden");
  } else {
    badgeDirtyState.classList.add("hidden");
  }
}

const titleMeasureCanvas = document.createElement("canvas");

function syncTitleInputWidth() {
  const val = inputProfileName.value || inputProfileName.placeholder || "";
  const ctx = titleMeasureCanvas.getContext("2d");
  if (ctx) {
    const computed = window.getComputedStyle(inputProfileName);
    ctx.font = `${computed.fontWeight} ${computed.fontSize} ${computed.fontFamily}`;
    const letterSpacing = parseFloat(computed.letterSpacing) || 0;
    const textWidth = Math.ceil(ctx.measureText(val).width + letterSpacing * val.length);
    inputProfileName.style.width = `${Math.max(textWidth + 16, 60)}px`;
  }
}

function renderSelectedProfile() {
  const profile = appState.profiles.find((p) => p.id === selectedProfileId);
  if (!profile) return;

  inputProfileName.title = profile.name;
  inputProfileName.disabled = profile.is_system_base;
  if (profile.is_system_base) {
    btnDeleteProfile.classList.add("hidden");
  } else {
    btnDeleteProfile.classList.remove("hidden");
  }

  if (profile.enabled) {
    badgeActiveState.textContent = "적용 중";
    badgeActiveState.className = "pill pill-live";
  } else {
    badgeActiveState.textContent = "비활성";
    badgeActiveState.className = "pill pill-neutral";
  }

  if (!(isDirty && editorProfileId === profile.id)) {
    inputProfileName.value = profile.name;
    syncTitleInputWidth();
    textarea.value = profile.content;
    setDirty(false);
    editorProfileId = profile.id;
  }
  updateLineNumbers();
  validateEditorContent();
}

function renderPortMappings() {
  portMappingSummary.innerHTML = "";
  const statuses = appState.port_statuses || [];
  if (statuses.length === 0) {
    return;
  }

  if (appState.proxy_running && appState.proxy_port) {
    const proxyBadge = document.createElement("span");
    proxyBadge.className = "route-chip mono num";
    proxyBadge.title =
      appState.proxy_port === 80
        ? "내장 리버스 프록시가 80번 포트에서 수신 중입니다"
        : `80번 포트 권한 제한으로 내장 프록시가 :${appState.proxy_port} 포트에서 수신 중입니다 (http://도메인:${appState.proxy_port})`;
    const proxyLabel = document.createElement("span");
    proxyLabel.className = "muted";
    proxyLabel.textContent = `프록시 :${appState.proxy_port}`;
    proxyBadge.appendChild(proxyLabel);
    portMappingSummary.appendChild(proxyBadge);
  }

  statuses.slice(0, 4).forEach((s) => {
    const chip = document.createElement("span");
    chip.className = "route-chip mono num";
    chip.title = s.alive
      ? `${s.mapping.domain} -> ${s.mapping.target_ip}:${s.mapping.target_port} (로컬 포트 응답 중)`
      : `${s.mapping.domain} -> ${s.mapping.target_ip}:${s.mapping.target_port} (로컬 포트 닫혀 있음)`;

    const dot = document.createElement("span");
    dot.className = s.alive ? "status-dot live" : "status-dot";

    const domain = document.createElement("span");
    domain.textContent = s.mapping.domain;

    const port = document.createElement("span");
    port.className = "muted";
    port.textContent = `:${s.mapping.target_port}`;

    chip.appendChild(dot);
    chip.appendChild(domain);
    chip.appendChild(port);
    portMappingSummary.appendChild(chip);
  });

  if (statuses.length > 4) {
    const moreChip = document.createElement("span");
    moreChip.className = "route-chip mono num";
    moreChip.textContent = `+${statuses.length - 4}`;
    portMappingSummary.appendChild(moreChip);
  }
}

function updateLineNumbers() {
  const count = textarea.value.split("\n").length;
  lineNumbers.innerHTML = Array.from({ length: count }, (_, i) => i + 1).join("<br>");
}

let validationTimer = null;
let validationSeq = 0;

function scheduleValidateEditorContent() {
  if (validationTimer) clearTimeout(validationTimer);
  validationTimer = setTimeout(() => {
    validateEditorContent();
  }, 120);
}

async function validateEditorContent() {
  if (!invoke) return;
  const seq = ++validationSeq;
  const issues = await invoke("validate_hosts_text", { content: textarea.value });
  if (seq !== validationSeq) return;
  const conflicts = appState.preview?.conflicts || [];

  let dotClass;
  let message;
  if (issues.length > 0) {
    const first = issues[0];
    validationSummary.className = "diag-status is-error";
    dotClass = "status-dot err";
    message = `${first.line_number}행 오류: ${first.reason}`;
  } else if (conflicts.length > 0) {
    const firstConflict = conflicts[0];
    validationSummary.className = "diag-status";
    dotClass = "status-dot";
    message = `중복 도메인 감지: ${firstConflict.domain} (${firstConflict.profiles.join(", ")})`;
  } else {
    validationSummary.className = "diag-status";
    dotClass = "status-dot ok";
    message = "문법 정상";
  }
  validationSummary.title = message;

  const dot = document.createElement("span");
  dot.className = dotClass;
  const text = document.createElement("span");
  text.textContent = message;
  validationSummary.replaceChildren(dot, text);
}

// Editor Events
textarea.addEventListener("input", () => {
  setDirty(true);
  updateLineNumbers();
  scheduleValidateEditorContent();
});

inputProfileName.addEventListener("input", () => {
  syncTitleInputWidth();
  setDirty(true);
});

textarea.addEventListener("scroll", () => {
  lineNumbers.scrollTop = textarea.scrollTop;
});

textarea.addEventListener("keydown", (e) => {
  if ((e.metaKey || e.ctrlKey) && e.key === "/") {
    e.preventDefault();
    toggleLineComments();
  } else if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "s") {
    e.preventDefault();
    saveCurrentProfile();
  }
});

function toggleLineComments() {
  const start = textarea.selectionStart;
  const end = textarea.selectionEnd;
  const val = textarea.value;

  const lineStart = val.lastIndexOf("\n", start - 1) + 1;
  let lineEnd = val.indexOf("\n", end);
  if (lineEnd === -1) lineEnd = val.length;

  const selectedBlock = val.slice(lineStart, lineEnd);
  const lines = selectedBlock.split("\n");
  const allCommented = lines.every((l) => l.trim().startsWith("#"));

  const toggled = lines
    .map((line) => (allCommented ? line.replace(/^(\s*)#\s?/, "$1") : `# ${line}`))
    .join("\n");

  textarea.setRangeText(toggled, lineStart, lineEnd, "select");
  setDirty(true);
  updateLineNumbers();
  validateEditorContent();
}

async function saveCurrentProfile() {
  if (!selectedProfileId) return;
  try {
    const snapshot = await invoke("update_profile", {
      id: selectedProfileId,
      name: inputProfileName.value.trim() || "이름 없는 프로필",
      content: textarea.value,
      applyImmediately: true,
    });
    setDirty(false);
    applySnapshot(snapshot);
    showToast("저장 및 시스템 hosts 반영 완료");
  } catch (err) {
    showToast(`저장 실패: ${err}`);
  }
}

// Quick Rule Adder Form
document.getElementById("quick-add-form").onsubmit = async (e) => {
  e.preventDefault();
  const ipField = document.getElementById("quick-ip");
  const domainField = document.getElementById("quick-domain");
  const commentField = document.getElementById("quick-comment");

  const ip = ipField.value.trim();
  const domain = domainField.value.trim();
  const comment = commentField.value.trim();
  if (!ip || !domain) return;

  const paddedIp = ip.padEnd(15, " ");
  const newLine = comment ? `${paddedIp} ${domain} # ${comment}` : `${paddedIp} ${domain}`;
  const current = textarea.value.trimEnd();
  textarea.value = current ? `${current}\n${newLine}\n` : `${newLine}\n`;

  domainField.value = "";
  commentField.value = "";
  updateLineNumbers();
  validateEditorContent();
  await saveCurrentProfile();
};

// Buttons & Dialog Wiring
document.getElementById("btn-save-profile").onclick = saveCurrentProfile;
document.getElementById("btn-toggle-comment").onclick = toggleLineComments;

document.getElementById("btn-new-profile").onclick = () => {
  newProfileNameInput.value = "";
  dialogNewProfile.showModal();
  newProfileNameInput.focus();
};

document.getElementById("btn-cancel-new").onclick = () => dialogNewProfile.close();
document.getElementById("btn-close-new").onclick = () => dialogNewProfile.close();

formNewProfile.onsubmit = async (e) => {
  e.preventDefault();
  const name = newProfileNameInput.value.trim();
  if (!name) return;
  dialogNewProfile.close();
  const snapshot = await invoke("create_profile", {
    name,
    content: `# ${name}\n127.0.0.1       local.project.dev\n`,
  });
  selectedProfileId = snapshot.profiles[snapshot.profiles.length - 1].id;
  applySnapshot(snapshot);
  showToast(`'${name}' 프로필이 생성되었습니다`);
};

btnDeleteProfile.onclick = async () => {
  if (!selectedProfileId) return;
  const profile = appState.profiles.find((p) => p.id === selectedProfileId);
  const confirmed = await askConfirm(
    "프로필 삭제",
    `'${profile?.name || ""}' 프로필을 삭제하시겠습니까? 삭제 후에는 되돌릴 수 없습니다.`,
    "삭제"
  );
  if (!confirmed) return;

  const snapshot = await invoke("delete_profile", { id: selectedProfileId });
  selectedProfileId = snapshot.profiles[0]?.id || null;
  applySnapshot(snapshot);
  showToast("프로필이 삭제되었습니다");
};

chkSingleMode.onchange = async () => {
  const snapshot = await invoke("set_switch_mode", {
    singleMode: chkSingleMode.checked,
  });
  applySnapshot(snapshot);
  showToast(chkSingleMode.checked ? "단일 전환 모드 켜짐" : "다중 병합 모드 켜짐");
};

btnGrantPerm.onclick = async () => {
  try {
    const snapshot = await invoke("grant_permission");
    applySnapshot(snapshot);
    showToast("즉시 전환 권한이 활성화되었습니다");
  } catch (err) {
    showToast(`권한 설정 취소됨`);
  }
};

document.getElementById("btn-preview-merged").onclick = () => {
  mergedPreviewCode.textContent = appState.preview?.hosts_content || "";
  dialogPreview.showModal();
};

document.getElementById("btn-close-preview").onclick = () => dialogPreview.close();

document.getElementById("btn-copy-preview").onclick = async () => {
  const text = appState.preview?.hosts_content || "";
  if (navigator.clipboard) {
    await navigator.clipboard.writeText(text);
    showToast("병합된 hosts 내용이 클립보드에 복사되었습니다");
  }
};

document.getElementById("btn-panic-restore").onclick = async () => {
  const confirmed = await askConfirm(
    "원본 백업으로 복구",
    "앱 최초 실행 시 백업해 둔 원본 /private/etc/hosts 파일로 즉시 복구하시겠습니까?",
    "원본 복구"
  );
  if (!confirmed) return;
  try {
    const snapshot = await invoke("panic_restore");
    applySnapshot(snapshot);
    showToast("원본 /etc/hosts 파일로 안전하게 복구되었습니다");
  } catch (err) {
    showToast(`복구 실패: ${err}`);
  }
};

// Updater
function showUpdateBanner(info) {
  pendingUpdate = info;
  updateText.textContent = `새 버전 v${info.latest_version} 출시`;
  updateBanner.classList.remove("hidden");
}

async function checkForUpdateManual(silentIfLatest = false) {
  if (!invoke) return;
  try {
    btnCheckUpdate.disabled = true;
    const info = await invoke("check_update");
    if (info.update_available) {
      showUpdateBanner(info);
      if (!silentIfLatest) {
        const ok = await askConfirm(
          `새 버전 v${info.latest_version} 업데이트`,
          `현재 버전(v${info.current_version})보다 새로운 버전이 출시되었습니다. 지금 자동으로 다운로드하여 설치하고 앱을 재시작하시겠습니까?`,
          "지금 업데이트"
        );
        if (ok) await triggerInstallUpdate();
      }
    } else if (!silentIfLatest) {
      showToast(`현재 최신 버전(v${info.current_version})을 사용 중입니다`);
    }
  } catch (err) {
    if (!silentIfLatest) {
      showToast(`업데이트 확인 실패: ${err}`);
    }
  } finally {
    btnCheckUpdate.disabled = false;
  }
}

async function triggerInstallUpdate() {
  if (!pendingUpdate || !pendingUpdate.download_url || !pendingUpdate.asset_name) return;
  try {
    btnInstallUpdate.textContent = "설치 중...";
    btnInstallUpdate.disabled = true;
    showToast("업데이트 패키지 다운로드 및 설치 중...");
    await invoke("install_update", {
      downloadUrl: pendingUpdate.download_url,
      assetName: pendingUpdate.asset_name,
    });
  } catch (err) {
    showToast(`자동 업데이트 실패: ${err}`);
    btnInstallUpdate.textContent = "업데이트";
    btnInstallUpdate.disabled = false;
  }
}

btnCheckUpdate.onclick = () => checkForUpdateManual(false);
btnInstallUpdate.onclick = triggerInstallUpdate;

if (listen) {
  listen("profiles-updated", () => loadSnapshot());
  listen("update-available", (event) => {
    if (event.payload) showUpdateBanner(event.payload);
  });
  listen("trigger-update-check", () => checkForUpdateManual(false));
}

window.addEventListener("DOMContentLoaded", loadSnapshot);
