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
                pollInterval = setInterval(() => fetchPoolData(false), 15000);
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

    if (!pwd || pwd.length < 4) {
        errBox.textContent = 'Password must be at least 4 characters long.';
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
async function fetchUpdates() {
    try {
        const res = await fetch('/api/updates').then(r => r.json());
        latestUpdateData = res;
        const badge = document.getElementById('update-badge');
        const badgeVer = document.getElementById('update-badge-ver');

        if (res.update_available && res.latest_version) {
            badgeVer.textContent = `v${res.latest_version}`;
            badge.style.display = 'inline-flex';
        } else {
            badge.style.display = 'none';
        }
    } catch (e) {
        console.error('Failed to query updates:', e);
    }
}

function openUpdateModal() {
    if (!latestUpdateData) return;
    const modal = document.getElementById('modal-update');
    document.getElementById('update-modal-ver-text').textContent = `DriveSpanner v${latestUpdateData.latest_version} is available for installation`;
    document.getElementById('update-installed-ver').textContent = `v${latestUpdateData.current_version}`;
    document.getElementById('update-latest-ver').textContent = `v${latestUpdateData.latest_version}`;
    document.getElementById('update-release-notes').textContent = latestUpdateData.release_notes || 'No release notes provided.';

    const dlBtn = document.getElementById('btn-download-update');
    if (latestUpdateData.download_url) {
        dlBtn.href = latestUpdateData.download_url;
        dlBtn.style.display = 'inline-flex';
    } else {
        dlBtn.style.display = 'none';
    }

    modal.classList.add('show');
}

function closeUpdateModal() {
    const modal = document.getElementById('modal-update');
    if (modal) modal.classList.remove('show');
}

async function checkUpdatesNow() {
    const checkBtn = document.getElementById('btn-check-updates-now');
    const originalText = checkBtn.textContent;
    checkBtn.textContent = 'Checking...';
    checkBtn.disabled = true;

    try {
        const res = await fetch('/api/updates/check', { method: 'POST' }).then(r => r.json());
        latestUpdateData = res;
        openUpdateModal();
        const badge = document.getElementById('update-badge');
        const badgeVer = document.getElementById('update-badge-ver');
        if (res.update_available && res.latest_version) {
            badgeVer.textContent = `v${res.latest_version}`;
            badge.style.display = 'inline-flex';
        } else {
            badge.style.display = 'none';
        }
    } catch (e) {
        alert('Failed to check for updates: ' + e);
    } finally {
        checkBtn.textContent = originalText;
        checkBtn.disabled = false;
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
}

// Modals
function setupModals() {
    const modalAddDrive = document.getElementById('modal-add-drive');
    const modalAddRule = document.getElementById('modal-add-rule');

    document.getElementById('btn-open-add-drive').onclick = () => modalAddDrive.classList.add('show');
    document.getElementById('modal-close-add-drive').onclick = () => modalAddDrive.classList.remove('show');

    document.getElementById('btn-open-add-rule').onclick = () => modalAddRule.classList.add('show');
    document.getElementById('modal-close-add-rule').onclick = () => modalAddRule.classList.remove('show');
    document.getElementById('btn-cancel-rule').onclick = () => modalAddRule.classList.remove('show');

    document.getElementById('btn-save-rule').onclick = async () => {
        const path = document.getElementById('input-rule-path').value.trim();
        const count = parseInt(document.getElementById('select-rule-count').value);
        const desc = document.getElementById('input-rule-desc').value.trim();

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
                    description: desc || null
                })
            });

            modalAddRule.classList.remove('show');
            document.getElementById('input-rule-path').value = '';
            fetchPoolData(true);
        } catch (e) {
            console.error(e);
        }
    };
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
