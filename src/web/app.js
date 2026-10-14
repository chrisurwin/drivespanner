// DriveSpanner - Web Dashboard Controller

let selectedDriveToAdd = null;
let currentExplorerPath = '/';
let lastDrivesFetchTime = 0;
let latestUpdateData = null;
let pollInterval = null;
let latestOverview = null;
let latestMemberDrives = [];

// Authenticated API wrapper
async function apiFetch(url, options = {}) {
    options.headers = options.headers || {};
    const token = localStorage.getItem('pf_token');
    if (token) {
        options.headers['Authorization'] = `Bearer ${token}`;
    }

    const res = await fetch(url, options);
    if (res.status === 401) {
        // Session invalid or auth required
        showLoginModal();
        throw new Error('Authentication required');
    }
    return res;
}

document.addEventListener('DOMContentLoaded', () => {
    setupTabNavigation();
    setupModals();
    setupActions();
    checkAuthStatus();
});

// Authentication Status & Lifecycle
async function checkAuthStatus() {
    try {
        const token = localStorage.getItem('pf_token');
        const headers = token ? { 'Authorization': `Bearer ${token}` } : {};
        const res = await fetch('/api/auth/status', { headers }).then(r => r.json());

        const setupModal = document.getElementById('modal-auth-setup');
        const loginModal = document.getElementById('modal-auth-login');
        const btnLogout = document.getElementById('btn-logout');

        if (res.setup_required) {
            setupModal.classList.add('show');
            loginModal.classList.remove('show');
            if (btnLogout) btnLogout.style.display = 'none';
        } else if (res.auth_enabled && !res.authenticated) {
            setupModal.classList.remove('show');
            loginModal.classList.add('show');
            if (btnLogout) btnLogout.style.display = 'none';
        } else {
            // Authenticated or auth disabled
            setupModal.classList.remove('show');
            loginModal.classList.remove('show');
            if (btnLogout) {
                btnLogout.style.display = res.auth_enabled ? 'inline-flex' : 'none';
            }

            fetchPoolData(true);
            fetchUpdates();

            // Set up polling intervals once authenticated
            if (!pollInterval) {
                pollInterval = setInterval(() => {
                    fetchPoolData(false);
                    const activeTab = document.querySelector('.nav-item.active');
                    if (activeTab && activeTab.getAttribute('data-tab') === 'settings') {
                        fetchCacheStatus();
                    }
                }, 15000);
                setInterval(fetchUpdates, 30 * 60 * 1000);
            }
        }
    } catch (e) {
        console.error('Failed to check auth status:', e);
        fetchPoolData(true);
    }
}

function showLoginModal() {
    const loginModal = document.getElementById('modal-auth-login');
    if (loginModal) loginModal.classList.add('show');
    const pwdInput = document.getElementById('input-login-password');
    if (pwdInput) pwdInput.focus();
}

async function submitSetupPassword() {
    const pwd = document.getElementById('input-setup-password').value;
    const confirm = document.getElementById('input-setup-confirm').value;
    const errBox = document.getElementById('setup-error-msg');

    if (!pwd || pwd.length < 8) {
        errBox.textContent = 'Password must be at least 8 characters long.';
        errBox.style.display = 'block';
        return;
    }
    if (pwd !== confirm) {
        errBox.textContent = 'Passwords do not match.';
        errBox.style.display = 'block';
        return;
    }

    errBox.style.display = 'none';
    const btnSubmit = document.getElementById('btn-submit-setup');
    const origText = btnSubmit ? btnSubmit.textContent : '';
    if (btnSubmit) {
        btnSubmit.disabled = true;
        btnSubmit.textContent = 'Saving password...';
    }

    try {
        const res = await fetch('/api/auth/setup', {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json',
                'Accept': 'application/json'
            },
            body: JSON.stringify({ password: pwd })
        });

        const rawText = await res.text();
        let data = null;
        try {
            data = JSON.parse(rawText);
        } catch (_) {
            console.error('Non-JSON response from /api/auth/setup:', rawText);
        }

        if (res.ok && data && data.success && data.token) {
            localStorage.setItem('pf_token', data.token);
            document.getElementById('modal-auth-setup').classList.remove('show');
            const btnLogout = document.getElementById('btn-logout');
            if (btnLogout) btnLogout.style.display = 'inline-flex';
            try { fetchPoolData(true); } catch (e) { console.error(e); }
            try { fetchUpdates(); } catch (e) { console.error(e); }
        } else {
            const msg = (data && (data.message || data.error)) || `Server returned ${res.status}: ${res.statusText || rawText.substring(0, 80)}`;
            errBox.textContent = msg;
            errBox.style.display = 'block';
        }
    } catch (e) {
        console.error('Network failure connecting to /api/auth/setup:', e);
        errBox.textContent = 'Network error connecting to server: ' + (e.message || 'Connection failed');
        errBox.style.display = 'block';
    } finally {
        if (btnSubmit) {
            btnSubmit.disabled = false;
            btnSubmit.textContent = origText;
        }
    }
}

async function submitLogin() {
    const pwd = document.getElementById('input-login-password').value;
    const errBox = document.getElementById('login-error-msg');

    if (!pwd) {
        errBox.textContent = 'Please enter your password.';
        errBox.style.display = 'block';
        return;
    }

    errBox.style.display = 'none';
    const btnLogin = document.getElementById('btn-submit-login');
    const origText = btnLogin ? btnLogin.textContent : '';
    if (btnLogin) {
        btnLogin.disabled = true;
        btnLogin.textContent = 'Signing in...';
    }

    try {
        const res = await fetch('/api/auth/login', {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json',
                'Accept': 'application/json'
            },
            body: JSON.stringify({ password: pwd })
        });

        const rawText = await res.text();
        let data = null;
        try {
            data = JSON.parse(rawText);
        } catch (_) {
            console.error('Non-JSON response from /api/auth/login:', rawText);
        }

        if (res.ok && data && data.success && data.token) {
            localStorage.setItem('pf_token', data.token);
            document.getElementById('modal-auth-login').classList.remove('show');
            document.getElementById('input-login-password').value = '';
            const btnLogout = document.getElementById('btn-logout');
            if (btnLogout) btnLogout.style.display = 'inline-flex';
            try { fetchPoolData(true); } catch (e) { console.error(e); }
            try { fetchUpdates(); } catch (e) { console.error(e); }
        } else {
            const msg = (data && (data.message || data.error)) || `Server returned ${res.status}: ${res.statusText || rawText.substring(0, 80)}`;
            errBox.textContent = msg;
            errBox.style.display = 'block';
        }
    } catch (e) {
        console.error('Network failure connecting to /api/auth/login:', e);
        errBox.textContent = 'Network error communicating with server: ' + (e.message || 'Connection failed');
        errBox.style.display = 'block';
    } finally {
        if (btnLogin) {
            btnLogin.disabled = false;
            btnLogin.textContent = origText;
        }
    }
}


async function handleLogout() {
    try {
        await apiFetch('/api/auth/logout', { method: 'POST' });
    } catch (e) {
        // Ignore network errors on logout
    }
    localStorage.removeItem('pf_token');
    const btnLogout = document.getElementById('btn-logout');
    if (btnLogout) btnLogout.style.display = 'none';
    showLoginModal();
}

// Update Checker Functions
function renderUpdateStatus(data) {
    if (!data) return;
    const badge = document.getElementById('update-badge');
    const badgeVer = document.getElementById('update-badge-ver');

    if (badge && badgeVer) {
        if (data.update_available && data.latest_version) {
            badgeVer.textContent = `v${data.latest_version}`;
            badge.style.display = 'inline-flex';
        } else {
            badge.style.display = 'none';
        }
    }

    // Update Service Control tab card
    const srvInst = document.getElementById('service-installed-ver');
    const srvLatest = document.getElementById('service-latest-ver');
    const srvChecked = document.getElementById('service-last-checked-ver');

    if (srvInst) srvInst.textContent = `v${data.current_version || '1.3.0'}`;
    if (srvLatest) {
        if (data.update_available && data.latest_version) {
            srvLatest.innerHTML = `<span style="color: var(--accent-indigo); font-weight: 600;">v${data.latest_version} (Update Available)</span>`;
        } else if (data.latest_version) {
            srvLatest.innerHTML = `<span style="color: var(--accent-green); font-weight: 600;">v${data.latest_version} (Up to Date)</span>`;
        } else if (data.error) {
            srvLatest.innerHTML = `<span style="color: var(--accent-amber); font-size: 11px;" title="${data.error}">${data.error.substring(0, 32)}...</span>`;
        } else {
            srvLatest.textContent = 'Up to Date';
        }
    }
    if (srvChecked) {
        if (data.last_checked) {
            const d = new Date(data.last_checked);
            srvChecked.textContent = isNaN(d.getTime()) ? data.last_checked : d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' }) + ' (' + d.toLocaleDateString() + ')';
        } else {
            srvChecked.textContent = 'Never';
        }
    }
}

async function fetchUpdates() {
    try {
        const res = await apiFetch('/api/updates').then(r => r.json());
        latestUpdateData = res;
        renderUpdateStatus(res);
    } catch (e) {
        console.error('Failed to query updates:', e);
    }
}

function openUpdateModal() {
    if (!latestUpdateData) return;
    const modal = document.getElementById('modal-update');
    const headerTitle = document.getElementById('update-modal-title');
    const headerSub = document.getElementById('update-modal-ver-text');
    const iconSpan = document.getElementById('update-modal-icon');
    const installedEl = document.getElementById('update-installed-ver');
    const latestEl = document.getElementById('update-latest-ver');
    const notesEl = document.getElementById('update-release-notes');
    const dlBtn = document.getElementById('btn-download-update');

    if (installedEl) installedEl.textContent = `v${latestUpdateData.current_version || '1.3.0'}`;
    if (latestEl) latestEl.textContent = latestUpdateData.latest_version ? `v${latestUpdateData.latest_version}` : 'Up to Date';
    if (notesEl) notesEl.textContent = latestUpdateData.release_notes || 'No release notes provided.';

    const downloadUrl = latestUpdateData.msi_download_url || latestUpdateData.release_url;

    if (latestUpdateData.update_available) {
        if (iconSpan) iconSpan.textContent = '🚀';
        if (headerTitle) headerTitle.textContent = 'Software Update Available';
        if (headerSub) {
            headerSub.textContent = `DriveSpanner v${latestUpdateData.latest_version} is ready to install`;
            headerSub.style.color = 'var(--accent-indigo)';
        }
        if (dlBtn) {
            dlBtn.href = downloadUrl || `https://github.com/chrisurwin/drivespanner/releases/latest`;
            dlBtn.textContent = 'Download MSI Installer';
            dlBtn.style.display = 'inline-flex';
        }
    } else if (latestUpdateData.error) {
        if (iconSpan) iconSpan.textContent = '⚠️';
        if (headerTitle) headerTitle.textContent = 'Update Check Notice';
        if (headerSub) {
            headerSub.textContent = latestUpdateData.error;
            headerSub.style.color = 'var(--accent-amber)';
        }
        if (dlBtn) {
            dlBtn.href = 'https://github.com/chrisurwin/drivespanner/releases';
            dlBtn.textContent = 'View Releases on GitHub';
            dlBtn.style.display = 'inline-flex';
        }
    } else {
        if (iconSpan) iconSpan.textContent = '✨';
        if (headerTitle) headerTitle.textContent = 'DriveSpanner is Up to Date';
        if (headerSub) {
            headerSub.textContent = `You are running the latest version (v${latestUpdateData.current_version})`;
            headerSub.style.color = 'var(--accent-green)';
        }
        if (dlBtn) {
            if (latestUpdateData.release_url) {
                dlBtn.href = latestUpdateData.release_url;
                dlBtn.textContent = 'View Release on GitHub';
                dlBtn.style.display = 'inline-flex';
            } else {
                dlBtn.style.display = 'none';
            }
        }
    }

    modal.classList.add('show');
}

function closeUpdateModal() {
    const modal = document.getElementById('modal-update');
    if (modal) modal.classList.remove('show');
}

async function checkUpdatesNow() {
    const checkBtn = document.getElementById('btn-check-updates-now');
    const serviceCheckBtn = document.getElementById('btn-service-check-updates');
    const originalText = checkBtn ? checkBtn.textContent : 'Check Again';

    if (checkBtn) {
        checkBtn.textContent = 'Checking...';
        checkBtn.disabled = true;
    }
    if (serviceCheckBtn) {
        serviceCheckBtn.textContent = 'Checking...';
        serviceCheckBtn.disabled = true;
    }

    try {
        const res = await apiFetch('/api/updates/check', { method: 'POST' }).then(r => r.json());
        latestUpdateData = res;
        renderUpdateStatus(res);
        openUpdateModal();
    } catch (e) {
        alert('Failed to check for updates: ' + e);
    } finally {
        if (checkBtn) {
            checkBtn.textContent = originalText;
            checkBtn.disabled = false;
        }
        if (serviceCheckBtn) {
            serviceCheckBtn.textContent = 'Check for Updates';
            serviceCheckBtn.disabled = false;
        }
    }
}

// Tab Navigation
function setupTabNavigation() {
    const navItems = document.querySelectorAll('.nav-item');
    const tabPanes = document.querySelectorAll('.tab-pane');
    const pageTitle = document.getElementById('page-title');

    navItems.forEach(item => {
        item.addEventListener('click', () => {
            const tabId = item.getAttribute('data-tab');
            navItems.forEach(i => i.classList.remove('active'));
            tabPanes.forEach(p => p.classList.remove('active'));

            item.classList.add('active');
            const targetPane = document.getElementById(`tab-${tabId}`);
            if (targetPane) targetPane.classList.add('active');

            if (tabId === 'overview') {
                pageTitle.textContent = 'Pool Overview';
                fetchPoolData(true);
            }
            if (tabId === 'replication') pageTitle.textContent = 'Folder Duplication';
            if (tabId === 'balancer') pageTitle.textContent = 'Drive Balancer';
            if (tabId === 'explorer') {
                pageTitle.textContent = 'Virtual Pool Explorer';
                loadExplorerPath(currentExplorerPath);
            }
            if (tabId === 'service') pageTitle.textContent = 'Windows Service Control';
            if (tabId === 'settings') {
                pageTitle.textContent = 'SSD Cache & Settings';
                fetchCacheStatus();
            }
        });
    });
}

// Format bytes
function formatBytes(bytes) {
    if (bytes === 0) return '0 B';
    const k = 1024;
    const sizes = ['B', 'KB', 'MB', 'GB', 'TB', 'PB'];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
}

// Fetch pool data with spindown protection
async function fetchPoolData(forceDrivesRefresh = false) {
    try {
        const now = Date.now();
        const shouldFetchDrives = forceDrivesRefresh || (now - lastDrivesFetchTime > 60000);

        const statusRes = await apiFetch('/api/status').then(r => r.json());
        renderOverview(statusRes.overview, statusRes.replication, statusRes.balancer);

        if (shouldFetchDrives) {
            lastDrivesFetchTime = now;
            const [drivesRes, rulesRes] = await Promise.all([
                apiFetch('/api/drives').then(r => r.json()),
                apiFetch('/api/rules').then(r => r.json()),
            ]);

            renderDrives(drivesRes.member_drives);
            renderRules(rulesRes.rules, rulesRes.default_replicas);
            renderBalancer(statusRes.balancer, drivesRes.member_drives);
            renderAvailableDrives(drivesRes.available_system_drives);
        }
    } catch (e) {
        console.error('Failed to fetch pool status:', e);
    }
}

// Render Overview
function renderOverview(overview, replication, balancer) {
    if (!overview) return;
    latestOverview = overview;

    document.getElementById('val-total-space').textContent = formatBytes(overview.total_bytes);
    document.getElementById('val-space-breakdown').textContent = 
        `${formatBytes(overview.used_bytes)} Used / ${formatBytes(overview.free_bytes)} Free (${overview.used_percent.toFixed(1)}%)`;
    
    document.getElementById('bar-pool-fill').style.width = `${overview.used_percent}%`;
    document.getElementById('val-disk-count').textContent = `${overview.member_drive_count} Drives`;
    document.getElementById('val-files-count').textContent = `${overview.total_files_count.toLocaleString()} Files Indexed`;
    
    document.getElementById('val-balance-variance').textContent = `${overview.pool_imbalance_percent.toFixed(1)}%`;

    const statusChip = document.getElementById('global-status-chip');
    const statusText = document.getElementById('global-status-text');
    statusText.textContent = overview.status_badge;

    const mountText = document.getElementById('sidebar-mount-text');
    mountText.textContent = `Mount: ${overview.mount_point}`;

    const mountBtnText = document.getElementById('mount-toggle-text');
    if (overview.is_mounted) {
        mountBtnText.textContent = `Mounted (${overview.mount_point})`;
        document.getElementById('btn-mount-toggle').classList.replace('btn-primary', 'btn-outline');
    } else {
        mountBtnText.textContent = `Mount Drive (${overview.mount_point})`;
        document.getElementById('btn-mount-toggle').classList.replace('btn-outline', 'btn-primary');
    }

    const cfgMountVal = document.getElementById('service-cfg-mount-val');
    if (cfgMountVal) cfgMountVal.textContent = `${overview.mount_point}\\`;
    const cfgMountStatus = document.getElementById('service-cfg-mount-status');
    if (cfgMountStatus) {
        cfgMountStatus.textContent = overview.is_mounted ? 'Mounted & Active' : 'Unmounted';
        cfgMountStatus.className = overview.is_mounted ? 'info-val text-green' : 'info-val';
    }
    const selectDriveElem = document.getElementById('select-pool-drive-letter');
    if (selectDriveElem && !selectDriveElem.dataset.userInteracted) {
        selectDriveElem.value = overview.mount_point;
    }

    // Replication widget
    const dupHealth = document.getElementById('val-duplication-health');
    if (overview.under_replicated_count > 0) {
        dupHealth.textContent = `${overview.under_replicated_count} Degraded`;
        dupHealth.style.color = 'var(--accent-amber)';
    } else {
        dupHealth.textContent = '100% Protected';
        dupHealth.style.color = 'var(--accent-green)';
    }

    const scrubBadge = document.getElementById('badge-scrub-status');
    if (replication && replication.is_scrubbing) {
        scrubBadge.textContent = 'Scrubbing...';
        scrubBadge.className = 'metric-footer-badge blue';
    } else {
        scrubBadge.textContent = 'Scrub Idle';
        scrubBadge.className = 'metric-footer-badge green';
    }
}

// Render Member Drives
function renderDrives(drives) {
    latestMemberDrives = drives || [];
    const container = document.getElementById('drives-container');
    if (!container) return;

    if (!drives || drives.length === 0) {
        container.innerHTML = `
            <div class="card" style="grid-column: 1 / -1; text-align: center; padding: 40px;">
                <h3>No Physical Drives in Pool</h3>
                <p style="color: var(--text-secondary); margin: 8px 0 20px;">Click "Add Drive to Pool" above to add existing hard drives or SSDs into this aggregate storage.</p>
                <button class="btn btn-primary" onclick="openAddDriveModal()">+ Add Your First Drive</button>
            </div>
        `;
        return;
    }

    container.innerHTML = drives.map(d => `
        <div class="drive-card">
            <div class="drive-card-header">
                <div class="drive-info">
                    <div class="drive-letter-badge">${d.drive_path.substring(0, 2)}</div>
                    <div class="drive-names">
                        <span class="drive-label">${d.volume_name || 'Physical Disk'}</span>
                        <span class="drive-fs">${d.filesystem} &bull; ${(d.pooldata_path || d.poolpart_path || '').split('\\').pop()}</span>
                    </div>
                </div>
                <span class="drive-status-badge ${d.online ? 'online' : 'offline'}">${d.online ? 'Online' : 'Offline'}</span>
            </div>

            <div class="storage-bar">
                <div class="storage-fill" style="width: ${d.used_percent}%;"></div>
            </div>

            <div class="drive-space-row">
                <span>Used: <span class="highlight">${formatBytes(d.used_bytes)}</span> (${d.used_percent.toFixed(1)}%)</span>
                <span>Free: <span class="highlight">${formatBytes(d.free_bytes)}</span></span>
            </div>

            <div class="drive-meta-row">
                <span>${d.file_count.toLocaleString()} Files</span>
                ${d.is_landing_zone ? '<span style="color: var(--accent-indigo); font-weight: 600;">SSD Landing Zone</span>' : '<span>Standard Member</span>'}
                <button class="btn btn-outline" style="padding: 4px 10px; font-size: 11px;" onclick="removeDrive('${d.id}')">Remove</button>
            </div>
        </div>
    `).join('');
}

// Render Duplication Rules
function renderRules(rules, defaultReplicas) {
    const tbody = document.getElementById('rules-table-body');
    if (!tbody) return;

    // Update pill buttons
    document.querySelectorAll('.replica-pill').forEach(btn => {
        const val = parseInt(btn.getAttribute('data-default'));
        btn.classList.toggle('active', val === defaultReplicas);
    });

    if (!rules || rules.length === 0) {
        tbody.innerHTML = `<tr><td colspan="6" class="text-center" style="padding: 24px; color: var(--text-muted);">No folder rules defined. Default duplication (${defaultReplicas}x) applies to all files.</td></tr>`;
        return;
    }

    tbody.innerHTML = rules.map(r => `
        <tr>
            <td class="folder-path-cell">
                <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"></path></svg>
                <span>${r.path_pattern}</span>
                ${r.pin_to_cache ? `<span class="badge" style="background: rgba(245, 158, 11, 0.15); color: #f59e0b; padding: 2px 7px; border-radius: 4px; font-size: 11px; margin-left: 8px; font-weight: 600;">⚡ Fast SSD Cache</span>` : ''}
            </td>
            <td>
                <span class="replica-badge r${r.replica_count}">${r.replica_count}x Replicas</span>
            </td>
            <td>${r.replica_count} Distinct Disks</td>
            <td style="color: var(--text-secondary);">${r.description || '-'}</td>
            <td><span class="badge" style="background: rgba(16, 185, 129, 0.12); color: var(--accent-green); padding: 2px 6px; border-radius: 4px; font-size: 11px;">Active</span></td>
            <td>
                <button class="btn btn-outline" style="padding: 4px 8px; font-size: 11px;" onclick="deleteRule('${r.id}')">Delete</button>
            </td>
        </tr>
    `).join('');
}

// Render Drive Balancer
function renderBalancer(balancer, memberDrives) {
    if (!balancer) return;

    document.getElementById('bal-state-text').textContent = balancer.state;
    document.getElementById('bal-speed-text').textContent = `${balancer.speed_mb_per_sec.toFixed(1)} MB/s`;
    document.getElementById('bal-total-files').textContent = balancer.total_files_moved.toLocaleString();
    document.getElementById('bal-total-bytes').textContent = formatBytes(balancer.total_bytes_moved);
    document.getElementById('bal-variance-metric').textContent = `${balancer.pool_imbalance_percent.toFixed(1)}%`;

    const fileText = document.getElementById('bal-file-text');
    if (balancer.current_file) {
        fileText.textContent = `Moving: ${balancer.current_file} (${formatBytes(balancer.current_file_bytes_moved)} / ${formatBytes(balancer.current_file_total_bytes)})`;
        const pct = balancer.current_file_total_bytes > 0 
            ? (balancer.current_file_bytes_moved / balancer.current_file_total_bytes) * 100 
            : 0;
        document.getElementById('bal-progress-fill').style.width = `${pct}%`;
    } else {
        fileText.textContent = balancer.state === 'Balancing' ? 'Analyzing migration chunks...' : 'No active file migration';
        document.getElementById('bal-progress-fill').style.width = '0%';
    }

    // Render Balancer Utilization Bars
    const chartContainer = document.getElementById('balancer-utilization-container');
    if (chartContainer && memberDrives) {
        chartContainer.innerHTML = memberDrives.map(d => `
            <div class="util-row">
                <span class="util-label">${d.drive_path.substring(0, 2)} (${d.volume_name})</span>
                <div class="util-bar-track">
                    <div class="util-bar-fill" style="width: ${d.used_percent}%;"></div>
                </div>
                <span class="util-pct">${d.used_percent.toFixed(1)}%</span>
            </div>
        `).join('');
    }
}

// Render Available System Drives
function renderAvailableDrives(available) {
    const list = document.getElementById('modal-available-drives');
    if (!list) return;

    const memberLetters = (latestMemberDrives || []).map(d => (d.drive_path || '').substring(0, 1).toUpperCase());
    const mountLetter = latestOverview ? (latestOverview.mount_point || '').substring(0, 1).toUpperCase() : '';

    const unpooled = (available || []).filter(a => {
        if (a.is_already_member) return false;
        const letter = (a.drive_letter || '').substring(0, 1).toUpperCase();
        if (memberLetters.includes(letter)) return false;
        if (mountLetter && letter === mountLetter) return false;
        return true;
    });

    if (unpooled.length === 0) {
        list.innerHTML = `<p style="color: var(--text-muted); padding: 12px;">No unpooled drives detected. All available disks are already in the pool or reserved for virtual mount!</p>`;
        return;
    }

    list.innerHTML = unpooled.map(d => `
        <div class="avail-drive-item ${selectedDriveToAdd === d.drive_letter ? 'selected' : ''}" onclick="selectDriveToAdd('${d.drive_letter}')">
            <div>
                <strong>${d.drive_letter} (${d.volume_name || 'Local Disk'})</strong>
                <div style="font-size: 11px; color: var(--text-muted);">${d.filesystem} &bull; ${formatBytes(d.free_bytes)} free of ${formatBytes(d.total_bytes)}</div>
            </div>
            <button class="btn btn-outline" style="padding: 4px 10px; font-size: 11px;">Select</button>
        </div>
    `).join('');
}

function selectDriveToAdd(letter) {
    selectedDriveToAdd = letter;
    const isLz = document.getElementById('chk-landing-zone').checked;
    addDrive(letter, isLz);
}

// Explorer Navigation
async function loadExplorerPath(path) {
    currentExplorerPath = path;
    const tbody = document.getElementById('explorer-table-body');
    const breadcrumbs = document.getElementById('explorer-breadcrumbs');

    breadcrumbs.innerHTML = `
        <span class="crumb ${path === '/' ? 'active' : ''}" onclick="loadExplorerPath('/')">/ (Pool Root)</span>
        ${path.split('/').filter(Boolean).map((seg, idx, arr) => {
            const sub = '/' + arr.slice(0, idx + 1).join('/');
            return ` &gt; <span class="crumb ${sub === path ? 'active' : ''}" onclick="loadExplorerPath('${sub}')">${seg}</span>`;
        }).join('')}
    `;

    try {
        const res = await apiFetch(`/api/browse?path=${encodeURIComponent(path)}`).then(r => r.json());
        if (!res.entries || res.entries.length === 0) {
            tbody.innerHTML = `<tr><td colspan="6" class="text-center" style="padding: 24px; color: var(--text-muted);">Folder is empty.</td></tr>`;
            return;
        }

        tbody.innerHTML = res.entries.map(e => `
            <tr>
                <td class="folder-path-cell" style="cursor: ${e.is_dir ? 'pointer' : 'default'};" onclick="${e.is_dir ? `loadExplorerPath('${e.relative_path}')` : ''}">
                    ${e.is_dir 
                        ? '<svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="#f59e0b" stroke-width="2"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"></path></svg>'
                        : '<svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="#38bdf8" stroke-width="2"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"></path><polyline points="14 2 14 8 20 8"></polyline></svg>'
                    }
                    <strong>${e.name}</strong>
                </td>
                <td>${e.is_dir ? 'Folder' : 'File'}</td>
                <td>${e.is_dir ? '-' : formatBytes(e.size)}</td>
                <td><span class="replica-badge r${e.target_replicas}">${e.actual_replicas}/${e.target_replicas}x</span></td>
                <td style="font-family: var(--font-mono); font-size: 11.5px;">${e.replica_disks.map(d => d.substring(0, 4)).join(', ')}</td>
                <td style="color: var(--text-muted); font-size: 12px;">${e.modified ? new Date(e.modified * 1000).toLocaleDateString() : '-'}</td>
            </tr>
        `).join('');
    } catch (e) {
        tbody.innerHTML = `<tr><td colspan="6" class="text-center" style="color: var(--accent-rose);">Error reading folder.</td></tr>`;
    }
}

// Actions
function setupActions() {
    document.getElementById('btn-refresh').addEventListener('click', () => fetchPoolData(true));

    document.getElementById('btn-mount-toggle').addEventListener('click', async () => {
        const isMounted = latestOverview && latestOverview.is_mounted;
        const mountPoint = latestOverview ? latestOverview.mount_point : 'V:';
        try {
            if (isMounted) {
                const res = await apiFetch('/api/unmount', { method: 'POST' }).then(r => r.json());
                alert(res.message);
            } else {
                const res = await apiFetch('/api/mount', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ drive_letter: mountPoint })
                }).then(r => r.json());
                alert(res.message);
            }
            fetchPoolData(true);
        } catch (e) {
            console.error(e);
            alert(e.message || 'Mount operation failed');
        }
    });

    const btnSaveMount = document.getElementById('btn-save-mount-letter');
    if (btnSaveMount) {
        btnSaveMount.addEventListener('click', async () => {
            const selectDrive = document.getElementById('select-pool-drive-letter');
            const newLetter = selectDrive ? selectDrive.value : 'V:';
            try {
                const res = await apiFetch('/api/mount/letter', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ drive_letter: newLetter })
                }).then(r => r.json());
                alert(res.message);
                fetchPoolData(true);
            } catch (e) {
                console.error(e);
                alert(e.message || 'Failed to update mount letter');
            }
        });
    }

    const selectDrive = document.getElementById('select-pool-drive-letter');
    if (selectDrive) {
        selectDrive.addEventListener('change', () => {
            selectDrive.dataset.userInteracted = 'true';
        });
    }

    document.getElementById('btn-trigger-scrub').addEventListener('click', async () => {
        try {
            await apiFetch('/api/scrub/start', { method: 'POST' });
            alert('Pool audit and replica resilvering started in background!');
            fetchPoolData(true);
        } catch (e) {
            console.error(e);
        }
    });

    document.getElementById('btn-start-balance').addEventListener('click', async () => {
        try {
            await apiFetch('/api/balance/start', { method: 'POST' });
            alert('Drive balancing started!');
            fetchPoolData(true);
        } catch (e) {
            console.error(e);
        }
    });

    document.getElementById('btn-stop-balance').addEventListener('click', async () => {
        try {
            await apiFetch('/api/balance/stop', { method: 'POST' });
            fetchPoolData(true);
        } catch (e) {
            console.error(e);
        }
    });

    document.querySelectorAll('.replica-pill').forEach(btn => {
        btn.addEventListener('click', async () => {
            const def = parseInt(btn.getAttribute('data-default'));
            try {
                await apiFetch('/api/rules/default', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ default_replicas: def })
                });
                fetchPoolData(true);
            } catch (e) {
                console.error(e);
            }
        });
    });

    // SSD Cache & Plex Listeners
    const btnSaveCache = document.getElementById('btn-save-cache-settings');
    if (btnSaveCache) btnSaveCache.addEventListener('click', saveCacheSettings);

    const btnSyncPlex = document.getElementById('btn-sync-plex-now');
    if (btnSyncPlex) btnSyncPlex.addEventListener('click', triggerPlexSync);

    const btnSyncPinned = document.getElementById('btn-sync-pinned-cache');
    if (btnSyncPinned) btnSyncPinned.addEventListener('click', triggerPinnedSync);

    const btnRunMover = document.getElementById('btn-run-mover');
    if (btnRunMover) btnRunMover.addEventListener('click', triggerCacheMover);

    const inputMoverThresh = document.getElementById('input-mover-threshold');
    if (inputMoverThresh) {
        inputMoverThresh.addEventListener('input', (e) => {
            const lbl = document.getElementById('mover-threshold-label');
            if (lbl) lbl.textContent = `${e.target.value}%`;
        });
    }
}

// Modals
function setupModals() {
    const modalAddDrive = document.getElementById('modal-add-drive');
    const modalAddRule = document.getElementById('modal-add-rule');
    const modalPinFolder = document.getElementById('modal-pin-folder');

    document.getElementById('btn-open-add-drive').onclick = () => modalAddDrive.classList.add('show');
    document.getElementById('modal-close-add-drive').onclick = () => modalAddDrive.classList.remove('show');

    document.getElementById('btn-open-add-rule').onclick = () => {
        if (document.getElementById('chk-rule-pin-cache')) document.getElementById('chk-rule-pin-cache').checked = false;
        modalAddRule.classList.add('show');
    };
    document.getElementById('modal-close-add-rule').onclick = () => modalAddRule.classList.remove('show');
    document.getElementById('btn-cancel-rule').onclick = () => modalAddRule.classList.remove('show');

    document.getElementById('btn-save-rule').onclick = async () => {
        const path = document.getElementById('input-rule-path').value.trim();
        const count = parseInt(document.getElementById('select-rule-count').value);
        const desc = document.getElementById('input-rule-desc').value.trim();
        const pinToCache = document.getElementById('chk-rule-pin-cache') ? document.getElementById('chk-rule-pin-cache').checked : false;

        if (!path) {
            alert('Please specify a folder path (e.g. /Documents)');
            return;
        }

        try {
            await apiFetch('/api/rules', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    path_pattern: path,
                    replica_count: count,
                    description: desc || null,
                    pin_to_cache: pinToCache
                })
            });

            modalAddRule.classList.remove('show');
            document.getElementById('input-rule-path').value = '';
            if (document.getElementById('chk-rule-pin-cache')) document.getElementById('chk-rule-pin-cache').checked = false;
            fetchPoolData(true);
            fetchCacheStatus();
        } catch (e) {
            console.error(e);
        }
    };

    // Fast Cache Pin Folder Modal
    const btnOpenPin = document.getElementById('btn-open-pin-folder-modal');
    const btnClosePin = document.getElementById('modal-close-pin-folder');
    const btnCancelPin = document.getElementById('btn-cancel-pin-folder');
    const btnSavePin = document.getElementById('btn-save-pin-folder');

    if (btnOpenPin && modalPinFolder) {
        btnOpenPin.onclick = () => {
            const pInput = document.getElementById('input-pin-path');
            const dInput = document.getElementById('input-pin-desc');
            if (pInput) pInput.value = '';
            if (dInput) dInput.value = '';
            modalPinFolder.classList.add('show');
        };
    }
    if (btnClosePin && modalPinFolder) {
        btnClosePin.onclick = () => modalPinFolder.classList.remove('show');
    }
    if (btnCancelPin && modalPinFolder) {
        btnCancelPin.onclick = () => modalPinFolder.classList.remove('show');
    }
    if (btnSavePin && modalPinFolder) {
        btnSavePin.onclick = async () => {
            const path = document.getElementById('input-pin-path').value.trim();
            const replicas = parseInt(document.getElementById('select-pin-replicas').value, 10);
            const desc = document.getElementById('input-pin-desc').value.trim();

            if (!path) {
                alert('Please enter a folder path (e.g. /Music)');
                return;
            }

            const origText = btnSavePin.textContent;
            try {
                btnSavePin.disabled = true;
                btnSavePin.textContent = 'Pinning & Syncing...';

                const res = await apiFetch('/api/cache/pinned', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({
                        path_pattern: path,
                        replica_count: replicas,
                        description: desc || null
                    })
                }).then(r => r.json());

                modalPinFolder.classList.remove('show');
                alert(res.message || 'Folder pinned to Fast SSD Cache and queued for sync!');
                fetchCacheStatus();
                fetchPoolData(true);
            } catch (e) {
                console.error('Failed to pin folder to fast cache:', e);
                alert(e.message || 'Failed to pin folder to fast cache');
            } finally {
                btnSavePin.disabled = false;
                btnSavePin.textContent = origText;
            }
        };
    }
}

function openAddDriveModal() {
    document.getElementById('modal-add-drive').classList.add('show');
}

async function addDrive(drivePath, isLandingZone) {
    try {
        const res = await apiFetch('/api/drives/add', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
                drive_path: drivePath,
                is_landing_zone: isLandingZone
            })
        }).then(r => r.json());

        document.getElementById('modal-add-drive').classList.remove('show');
        alert(res.message);
        fetchPoolData(true);
    } catch (e) {
        console.error(e);
        alert(e.message || 'Failed to add drive to storage pool');
    }
}

async function removeDrive(diskId) {
    if (!confirm('Are you sure you want to remove this disk from the storage pool?')) return;
    try {
        const res = await apiFetch('/api/drives/remove', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ disk_id: diskId })
        }).then(r => r.json());
        alert(res.message);
        fetchPoolData(true);
    } catch (e) {
        console.error(e);
    }
}

async function deleteRule(ruleId) {
    if (!confirm('Delete this folder duplication rule?')) return;
    try {
        await apiFetch(`/api/rules/${ruleId}`, { method: 'DELETE' });
        fetchPoolData(true);
    } catch (e) {
        console.error(e);
    }
}

// SSD Cache Tier & Plex Integration Controllers
async function fetchCacheStatus() {
    try {
        const res = await apiFetch('/api/cache/status').then(r => r.json());
        if (res && res.success) {
            renderCacheStatus(res);
        }
    } catch (e) {
        console.error('Failed to fetch cache and Plex status:', e);
    }
}

function renderCacheStatus(data) {
    if (!data) return;

    // 1. Render SSD Cache Disk
    const driveDisplay = document.getElementById('cache-drive-display');
    const capDisplay = document.getElementById('cache-capacity-display');
    const usedText = document.getElementById('cache-used-text');
    const freeText = document.getElementById('cache-free-text');
    const barFill = document.getElementById('bar-cache-fill');
    const tierBadge = document.getElementById('cache-tier-badge');

    if (data.cache_disk) {
        const cd = data.cache_disk;
        if (driveDisplay) driveDisplay.textContent = `${cd.drive_path} (${cd.volume_name || 'SSD Cache Drive'})`;
        if (capDisplay) capDisplay.textContent = formatBytes(cd.total_bytes);
        if (usedText) usedText.textContent = `Used: ${formatBytes(cd.used_bytes)} (${cd.used_percent.toFixed(1)}%)`;
        if (freeText) freeText.textContent = `Free: ${formatBytes(cd.free_bytes)}`;
        if (barFill) barFill.style.width = `${cd.used_percent}%`;
        if (tierBadge) {
            tierBadge.textContent = 'Active Tier (Online)';
            tierBadge.style.color = 'var(--accent-green)';
            tierBadge.style.background = 'rgba(16, 185, 129, 0.15)';
        }
    } else {
        if (driveDisplay) driveDisplay.textContent = 'No SSD Cache drive configured';
        if (capDisplay) capDisplay.textContent = '0 GB';
        if (usedText) usedText.textContent = 'Used: 0 GB';
        if (freeText) freeText.textContent = 'Free: 0 GB';
        if (barFill) barFill.style.width = '0%';
        if (tierBadge) {
            tierBadge.textContent = 'Unassigned';
            tierBadge.style.color = 'var(--accent-amber)';
            tierBadge.style.background = 'rgba(245, 158, 11, 0.15)';
        }
    }

    // 2. Render Settings Form
    if (data.config) {
        const cfg = data.config;
        const chkPlex = document.getElementById('chk-plex-enabled');
        const chkPrecacheNext = document.getElementById('chk-plex-precache-next');
        const chkEvictWatched = document.getElementById('chk-plex-evict-watched');
        const inputUrl = document.getElementById('input-plex-url');
        const inputQuota = document.getElementById('input-plex-quota');
        const inputThresh = document.getElementById('input-mover-threshold');
        const threshLbl = document.getElementById('mover-threshold-label');

        if (chkPlex && !chkPlex.dataset.userEdited) chkPlex.checked = !!cfg.plex.enabled;
        if (chkPrecacheNext && !chkPrecacheNext.dataset.userEdited) chkPrecacheNext.checked = !!cfg.plex.precache_next_episode;
        if (chkEvictWatched && !chkEvictWatched.dataset.userEdited) chkEvictWatched.checked = cfg.plex.evict_watched_episodes !== false;
        if (inputUrl && !inputUrl.dataset.userEdited) inputUrl.value = cfg.plex.server_url || 'http://localhost:32400';
        if (inputQuota && !inputQuota.dataset.userEdited) inputQuota.value = cfg.plex.max_cache_gb || 50;
        if (inputThresh && !inputThresh.dataset.userEdited) inputThresh.value = cfg.move_threshold_percent || 80;
        if (threshLbl) threshLbl.textContent = `${cfg.move_threshold_percent || 80}%`;
    }

    // 2.5. Render Fast Cache Pinned Folders
    const pinnedTbody = document.getElementById('pinned-cache-table-body');
    if (pinnedTbody) {
        const pinnedList = data.pinned_folders || [];
        if (pinnedList.length === 0) {
            pinnedTbody.innerHTML = `<tr><td colspan="6" class="text-center" style="padding: 24px; color: var(--text-muted);">No folders currently pinned to Fast Cache. Click '+ Pin Folder' to add folders like /Music for instant playback with array duplication.</td></tr>`;
        } else {
            pinnedTbody.innerHTML = pinnedList.map(item => `
                <tr>
                    <td class="folder-path-cell" style="font-weight: 600; color: var(--text-primary);">
                        <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"></path></svg>
                        <span>${item.path_pattern}</span>
                        ${item.description ? `<span style="font-size: 11px; font-weight: normal; color: var(--text-secondary); margin-left: 6px;">(${item.description})</span>` : ''}
                    </td>
                    <td>
                        <span class="replica-badge r${item.replica_count}">${item.replica_count}x Replicas (HDDs)</span>
                    </td>
                    <td style="color: #f59e0b; font-weight: 600;">
                        ${formatBytes(item.cached_bytes)} <span style="font-size: 11.5px; font-weight: normal; color: var(--text-secondary);">(${item.cached_files.toLocaleString()} files)</span>
                    </td>
                    <td style="color: var(--text-secondary);">
                        ${formatBytes(item.total_bytes)} <span style="font-size: 11.5px; color: var(--text-muted);">(${item.total_files.toLocaleString()} files)</span>
                    </td>
                    <td>
                        ${item.is_fully_cached 
                            ? `<span class="badge" style="background: rgba(16, 185, 129, 0.12); color: var(--accent-green); padding: 2px 7px; border-radius: 4px; font-size: 11px; font-weight: 600;">⚡ 100% Cached</span>`
                            : `<span class="badge" style="background: rgba(245, 158, 11, 0.12); color: #f59e0b; padding: 2px 7px; border-radius: 4px; font-size: 11px; font-weight: 600;">Syncing (${item.cached_files}/${item.total_files})</span>`
                        }
                    </td>
                    <td class="text-right">
                        <button class="btn btn-outline" style="padding: 4px 8px; font-size: 11px;" onclick="unpinFolder('${item.id}')">Unpin</button>
                    </td>
                </tr>
            `).join('');
        }
    }

    // 3. Render Plex Pre-caching Status & Items
    if (data.plex_status) {
        const ps = data.plex_status;
        const statusDot = document.getElementById('plex-status-dot');
        const statusText = document.getElementById('plex-status-text');
        const lastSyncLbl = document.getElementById('plex-last-sync-label');

        if (ps.connected) {
            if (statusDot) statusDot.style.background = 'var(--accent-green)';
            if (statusText) {
                statusText.textContent = 'Connected & Active';
                statusText.style.color = 'var(--accent-green)';
            }
        } else if (ps.error_message) {
            if (statusDot) statusDot.style.background = 'var(--accent-amber)';
            if (statusText) {
                statusText.textContent = `Sync Error: ${ps.error_message}`;
                statusText.style.color = 'var(--accent-amber)';
            }
        } else {
            if (statusDot) statusDot.style.background = 'var(--text-muted)';
            if (statusText) {
                statusText.textContent = 'Awaiting Initial Sync';
                statusText.style.color = 'var(--text-secondary)';
            }
        }

        if (lastSyncLbl) {
            if (ps.last_sync_time) {
                const clean = ps.last_sync_time.replace('-sync', '');
                const ts = parseInt(clean, 10);
                if (!isNaN(ts) && ts > 0) {
                    const d = new Date(ts * 1000);
                    lastSyncLbl.textContent = `Last sync: ${d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' })}`;
                } else {
                    lastSyncLbl.textContent = `Last sync: ${ps.last_sync_time}`;
                }
            } else {
                lastSyncLbl.textContent = 'Last sync: Never';
            }
        }

        const itemsBadge = document.getElementById('plex-items-count-badge');
        if (itemsBadge) {
            itemsBadge.textContent = `${ps.items.length} Items (${formatBytes(ps.total_cached_bytes)})`;
        }

        const evictedBadge = document.getElementById('plex-evicted-count-badge');
        if (evictedBadge) {
            if (ps.evicted_items_count && ps.evicted_items_count > 0) {
                evictedBadge.textContent = `${ps.evicted_items_count} Returned to HDD`;
                evictedBadge.style.display = 'inline-block';
                if (ps.last_evicted_summary) {
                    evictedBadge.title = `Last returned: ${ps.last_evicted_summary}`;
                }
            } else {
                evictedBadge.style.display = 'none';
            }
        }

        const tbody = document.getElementById('plex-cached-table-body');
        if (tbody) {
            if (!ps.items || ps.items.length === 0) {
                tbody.innerHTML = `<tr><td colspan="6" class="text-center" style="padding: 24px; color: var(--text-muted);">No media items currently pre-cached on the SSD. Click 'Sync Plex Now' to query Continue Watching.</td></tr>`;
            } else {
                tbody.innerHTML = ps.items.map(item => `
                    <tr>
                        <td style="font-weight: 600; color: var(--text-primary);">${item.title}</td>
                        <td><span class="badge" style="background: rgba(99, 102, 241, 0.12); color: var(--accent-indigo); padding: 2px 6px; border-radius: 4px; font-size: 11px;">${item.media_type}</span></td>
                        <td style="color: var(--accent-blue);">${item.source}</td>
                        <td>${formatBytes(item.size_bytes)}</td>
                        <td style="font-family: var(--font-mono); font-size: 11.5px; color: var(--text-secondary);">${item.rel_path}</td>
                        <td><span class="badge" style="background: rgba(16, 185, 129, 0.12); color: var(--accent-green); padding: 2px 6px; border-radius: 4px; font-size: 11px;">Cached on SSD</span></td>
                    </tr>
                `).join('');
            }
        }
    }
}

async function unpinFolder(ruleId) {
    if (!confirm('Unpin this folder from Fast SSD Cache? Existing files on SSD will be migrated to the HDD array on the next mover cycle, but your HDD array replicas remain protected.')) {
        return;
    }
    try {
        const res = await apiFetch(`/api/cache/pinned/unpin/${ruleId}`, {
            method: 'POST'
        }).then(r => r.json());
        alert(res.message || 'Folder unpinned from Fast SSD Cache.');
        fetchCacheStatus();
        fetchPoolData(true);
    } catch (e) {
        console.error('Failed to unpin folder:', e);
        alert(e.message || 'Failed to unpin folder');
    }
}

async function triggerPinnedSync() {
    const btn = document.getElementById('btn-sync-pinned-cache');
    const origText = btn ? btn.textContent : '';
    if (btn) {
        btn.disabled = true;
        btn.textContent = 'Syncing Fast Cache...';
    }
    try {
        const res = await apiFetch('/api/cache/pinned/sync', {
            method: 'POST'
        }).then(r => r.json());
        alert(res.message || 'Fast SSD Cache synchronization complete.');
        fetchCacheStatus();
    } catch (e) {
        console.error('Failed to sync fast cache:', e);
        alert(e.message || 'Failed to sync fast cache');
    } finally {
        if (btn) {
            btn.disabled = false;
            btn.textContent = origText;
        }
    }
}

async function saveCacheSettings() {
    const btn = document.getElementById('btn-save-cache-settings');
    const origText = btn ? btn.textContent : '';
    if (btn) {
        btn.disabled = true;
        btn.textContent = 'Saving...';
    }

    try {
        const enabled = document.getElementById('chk-plex-enabled').checked;
        const precacheNext = document.getElementById('chk-plex-precache-next').checked;
        const evictWatched = document.getElementById('chk-plex-evict-watched').checked;
        const serverUrl = document.getElementById('input-plex-url').value.trim();
        const token = document.getElementById('input-plex-token').value.trim();
        const maxGb = parseFloat(document.getElementById('input-plex-quota').value) || 50;
        const thresh = parseFloat(document.getElementById('input-mover-threshold').value) || 80;

        const res = await apiFetch('/api/cache/config', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
                enabled: true,
                move_threshold_percent: thresh,
                plex_enabled: enabled,
                plex_precache_next_episode: precacheNext,
                plex_evict_watched_episodes: evictWatched,
                plex_server_url: serverUrl,
                plex_token: token ? token : null,
                plex_max_cache_gb: maxGb
            })
        }).then(r => r.json());

        alert(res.message || 'Cache and Plex settings saved successfully.');
        fetchCacheStatus();
    } catch (e) {
        console.error('Failed to save cache settings:', e);
        alert(e.message || 'Failed to save cache settings');
    } finally {
        if (btn) {
            btn.disabled = false;
            btn.textContent = origText;
        }
    }
}

async function triggerPlexSync() {
    const btn = document.getElementById('btn-sync-plex-now');
    const origText = btn ? btn.textContent : '';
    if (btn) {
        btn.disabled = true;
        btn.textContent = 'Syncing with Plex...';
    }

    try {
        const res = await apiFetch('/api/cache/sync', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' }
        }).then(r => r.json());

        alert(res.message || 'Plex Continue Watching synchronization complete.');
        fetchCacheStatus();
    } catch (e) {
        console.error('Failed to sync with Plex:', e);
        alert(e.message || 'Failed to sync with Plex');
    } finally {
        if (btn) {
            btn.disabled = false;
            btn.textContent = origText;
        }
    }
}

async function triggerCacheMover() {
    const btn = document.getElementById('btn-run-mover');
    const origText = btn ? btn.textContent : '';
    if (btn) {
        btn.disabled = true;
        btn.textContent = 'Moving files...';
    }

    try {
        const res = await apiFetch('/api/cache/move', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' }
        }).then(r => r.json());

        alert(res.message || 'SSD Cache Mover complete.');
        fetchCacheStatus();
        fetchPoolData(true);
    } catch (e) {
        console.error('Failed to run cache mover:', e);
        alert(e.message || 'Failed to run cache mover');
    } finally {
        if (btn) {
            btn.disabled = false;
            btn.textContent = origText;
        }
    }
}
