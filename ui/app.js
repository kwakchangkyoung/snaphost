const invoke = window.__TAURI__?.core?.invoke;
const listen = window.__TAURI__?.event?.listen;

let appState = {
  profiles: [],
  single_switch_mode: false,
  status: null,
  preview: null,
};
let selectedProfileId = null;

// DOM Elements
const profileListEl = document.getElementById("profile-list");
const chkSingleMode = document.getElementById("chk-single-mode");
const inputProfileName = document.getElementById("input-profile-name");
const badgeActiveState = document.getElementById("badge-active-state");
const textarea = document.getElementById("hosts-textarea");
const lineNumbers = document.getElementById("line-numbers");
const validationSummary = document.getElementById("validation-summary");
const portMappingSummary = document.getElementById("port-mapping-summary");
const permDot = document.getElementById("perm-dot");
const permText = document.getElementById("perm-text");
const btnGrantPerm = document.getElementById("btn-grant-perm");
const btnDeleteProfile = document.getElementById("btn-delete-profile");
const modalPreview = document.getElementById("modal-preview");
const mergedPreviewCode = document.getElementById("merged-preview-code");
const appVersionEl = document.getElementById("app-version");
const updateBanner = document.getElementById("update-banner");
const updateText = document.getElementById("update-text");
const btnInstallUpdate = document.getElementById("btn-install-update");
const btnCheckUpdate = document.getElementById("btn-check-update");

let pendingUpdate = null;

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

  renderSidebar();
  renderPermissionStatus();
  renderSelectedProfile();
  renderPortMappings();
}

function renderPermissionStatus() {
  if (!appState.status) return;
  if (appState.status.hosts_writable) {
    permDot.className = "dot ok";
    permText.textContent = "즉시 전환(0ms) 활성화됨";
    btnGrantPerm.style.display = "none";
  } else {
    permDot.className = "dot";
    permText.textContent = "/etc/hosts 쓰기 권한 필요";
    btnGrantPerm.style.display = "inline-block";
  }
}

function renderSidebar() {
  profileListEl.innerHTML = "";
  appState.profiles.forEach((profile) => {
    const li = document.createElement("li");
    li.className = `profile-item ${profile.id === selectedProfileId ? "selected" : ""}`;
    li.onclick = (e) => {
      if (e.target.tagName === "INPUT" || e.target.classList.contains("slider")) return;
      selectedProfileId = profile.id;
      renderSidebar();
      renderSelectedProfile();
    };

    const left = document.createElement("div");
    left.className = "profile-left";

    const nameSpan = document.createElement("span");
    nameSpan.className = "profile-name";
    nameSpan.textContent = profile.name;
    left.appendChild(nameSpan);

    if (profile.is_system_base) {
      const baseTag = document.createElement("span");
      baseTag.className = "tag-base";
      baseTag.textContent = "BASE";
      left.appendChild(baseTag);
    }

    const label = document.createElement("label");
    label.className = "switch";
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
      } catch (err) {
        alert("프로필 전환 실패: " + err);
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

function renderSelectedProfile() {
  const profile = appState.profiles.find((p) => p.id === selectedProfileId);
  if (!profile) return;

  inputProfileName.value = profile.name;
  inputProfileName.disabled = profile.is_system_base;
  btnDeleteProfile.style.display = profile.is_system_base ? "none" : "inline-block";

  if (profile.enabled) {
    badgeActiveState.textContent = "활성 (적용 중)";
    badgeActiveState.className = "badge active";
  } else {
    badgeActiveState.textContent = "비활성";
    badgeActiveState.className = "badge";
  }

  textarea.value = profile.content;
  updateLineNumbers();
  validateEditorContent();
}

function renderPortMappings() {
  const statuses = appState.port_statuses || [];
  if (statuses.length === 0) {
    portMappingSummary.innerHTML = "";
    return;
  }
  const proxyLabel = appState.proxy_running
    ? `프록시(:${appState.proxy_port})`
    : "포트 매핑";
  portMappingSummary.innerHTML =
    `<span>${proxyLabel}:</span>` +
    statuses
      .slice(0, 4)
      .map((s) => {
        const icon = s.alive ? "🟢" : "⚪";
        return `<span class="port-pill" title="${s.alive ? "로컬 서버 응답 중" : "로컬 포트 닫혀 있음"}">${icon} ${s.mapping.domain} → :${s.mapping.target_port}</span>`;
      })
      .join("");
}

function updateLineNumbers() {
  const count = textarea.value.split("\n").length;
  lineNumbers.innerHTML = Array.from({ length: count }, (_, i) => i + 1).join("<br>");
}

async function validateEditorContent() {
  if (!invoke) return;
  const issues = await invoke("validate_hosts_text", { content: textarea.value });
  if (issues.length === 0) {
    validationSummary.innerHTML = `<span class="diag-ok">✓ 문법 이상 없음</span>`;
  } else {
    const first = issues[0];
    validationSummary.innerHTML = `<span class="diag-err">⚠ ${first.line_number}번째 줄: ${first.reason} (외 ${issues.length - 1}건)</span>`;
  }
}

// Sync scroll & line numbers
textarea.addEventListener("input", () => {
  updateLineNumbers();
  validateEditorContent();
});

textarea.addEventListener("scroll", () => {
  lineNumbers.scrollTop = textarea.scrollTop;
});

// Keyboard shortcuts: Cmd + / (toggle comment) and Cmd + S (save)
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
    .map((line) => {
      if (allCommented) {
        return line.replace(/^(\s*)#\s?/, "$1");
      } else {
        return `# ${line}`;
      }
    })
    .join("\n");

  textarea.setRangeText(toggled, lineStart, lineEnd, "select");
  updateLineNumbers();
  validateEditorContent();
}

async function saveCurrentProfile() {
  if (!selectedProfileId) return;
  try {
    const snapshot = await invoke("update_profile", {
      id: selectedProfileId,
      name: inputProfileName.value.trim() || "Untitled",
      content: textarea.value,
      applyImmediately: true,
    });
    applySnapshot(snapshot);
  } catch (err) {
    alert("저장 실패: " + err);
  }
}

// Button Event Listeners
document.getElementById("btn-save-profile").onclick = saveCurrentProfile;

document.getElementById("btn-new-profile").onclick = async () => {
  const name = prompt("새 프로필 이름을 입력하세요:", "새 로컬 프로젝트");
  if (!name) return;
  const snapshot = await invoke("create_profile", {
    name,
    content: `# ${name}\n127.0.0.1       local.project.dev\n`,
  });
  selectedProfileId = snapshot.profiles[snapshot.profiles.length - 1].id;
  applySnapshot(snapshot);
};

btnDeleteProfile.onclick = async () => {
  if (!selectedProfileId) return;
  if (!confirm("이 프로필을 삭제하시겠습니까?")) return;
  const snapshot = await invoke("delete_profile", { id: selectedProfileId });
  selectedProfileId = snapshot.profiles[0]?.id || null;
  applySnapshot(snapshot);
};

chkSingleMode.onchange = async () => {
  const snapshot = await invoke("set_switch_mode", {
    singleMode: chkSingleMode.checked,
  });
  applySnapshot(snapshot);
};

btnGrantPerm.onclick = async () => {
  try {
    const snapshot = await invoke("grant_permission");
    applySnapshot(snapshot);
  } catch (err) {
    alert("권한 설정이 취소되었거나 실패했습니다: " + err);
  }
};

document.getElementById("btn-preview-merged").onclick = () => {
  mergedPreviewCode.textContent = appState.preview?.hosts_content || "";
  modalPreview.classList.remove("hidden");
};

document.getElementById("btn-close-modal").onclick = () => {
  modalPreview.classList.add("hidden");
};

document.getElementById("btn-panic-restore").onclick = async () => {
  if (!confirm("원본 /etc/hosts 백업 파일로 즉시 복구하시겠습니까?")) return;
  try {
    const snapshot = await invoke("panic_restore");
    applySnapshot(snapshot);
    alert("원본 /etc/hosts 상태로 안전하게 복구되었습니다.");
  } catch (err) {
    alert("복구 실패: " + err);
  }
};

function showUpdateBanner(info) {
  pendingUpdate = info;
  updateText.textContent = `🎉 새 버전 v${info.latest_version} 출시!`;
  updateBanner.classList.remove("hidden");
}

async function checkForUpdateManual(silentIfLatest = false) {
  if (!invoke) return;
  try {
    btnCheckUpdate.textContent = "🔄 버전 확인 중...";
    btnCheckUpdate.disabled = true;
    const info = await invoke("check_update");
    if (info.update_available) {
      showUpdateBanner(info);
      if (!silentIfLatest) {
        if (confirm(`새로운 버전(v${info.latest_version})이 있습니다!\n지금 바로 다운로드하여 자동 업데이트하시겠습니까?`)) {
          await triggerInstallUpdate();
        }
      }
    } else if (!silentIfLatest) {
      alert(`현재 최신 버전(v${info.current_version})을 사용 중입니다.`);
    }
  } catch (err) {
    if (!silentIfLatest) {
      alert("업데이트 확인 실패: " + err);
    }
  } finally {
    btnCheckUpdate.textContent = "🔄 GitHub 신규 버전 확인";
    btnCheckUpdate.disabled = false;
  }
}

async function triggerInstallUpdate() {
  if (!pendingUpdate) return;
  if (!pendingUpdate.download_url || !pendingUpdate.asset_name) {
    alert("릴리즈 페이지에서 설치 파일을 다운로드해 주세요: " + pendingUpdate.release_url);
    return;
  }
  try {
    btnInstallUpdate.textContent = "설치 중...";
    btnInstallUpdate.disabled = true;
    await invoke("install_update", {
      downloadUrl: pendingUpdate.download_url,
      assetName: pendingUpdate.asset_name,
    });
  } catch (err) {
    alert("자동 업데이트 실패: " + err);
    btnInstallUpdate.textContent = "자동 업데이트";
    btnInstallUpdate.disabled = false;
  }
}

btnCheckUpdate.onclick = () => checkForUpdateManual(false);
btnInstallUpdate.onclick = triggerInstallUpdate;

if (listen) {
  listen("profiles-updated", () => loadSnapshot());
  listen("update-available", (event) => {
    if (event.payload) {
      showUpdateBanner(event.payload);
    }
  });
  listen("trigger-update-check", () => {
    checkForUpdateManual(false);
  });
}

window.addEventListener("DOMContentLoaded", loadSnapshot);
