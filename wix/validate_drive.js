// PoolForge WiX Installer - Drive Letter Validator and Configurator

function InitDriveLetter() {
    try {
        var current = Session.Property("POOL_DRIVE_LETTER");
        var fso = new ActiveXObject("Scripting.FileSystemObject");

        if (!current || current === "") {
            // Check if V: is available by default
            if (!fso.DriveExists("V")) {
                Session.Property("POOL_DRIVE_LETTER") = "V:";
                Session.Property("DRIVE_VALID") = "1";
                return 1;
            }

            // Search for first available letter from Z down to D
            var candidates = ["Z", "Y", "X", "W", "U", "T", "S", "R", "Q", "P", "O", "N", "M", "L", "K", "J", "I", "H", "G", "F", "E", "D"];
            for (var i = 0; i < candidates.length; i++) {
                if (!fso.DriveExists(candidates[i])) {
                    Session.Property("POOL_DRIVE_LETTER") = candidates[i] + ":";
                    Session.Property("DRIVE_VALID") = "1";
                    return 1;
                }
            }
            Session.Property("POOL_DRIVE_LETTER") = "V:";
        }
        Session.Property("DRIVE_VALID") = "1";
    } catch (e) {
        // Fallback safely
        Session.Property("POOL_DRIVE_LETTER") = "V:";
        Session.Property("DRIVE_VALID") = "1";
    }
    return 1;
}

function ValidateDriveLetter() {
    try {
        var raw = Session.Property("POOL_DRIVE_LETTER");
        if (!raw || raw.replace(/\s+/g, '') === "") {
            Session.Property("DRIVE_VALID") = "0";
            Session.Property("DRIVE_ERROR_MSG") = "Please enter a drive letter (e.g. V:).";
            return 1;
        }

        var letter = raw.replace(/\s+/g, '').toUpperCase();
        if (letter.length === 1) {
            letter = letter + ":";
        }

        if (letter.length !== 2 || letter.charAt(1) !== ':') {
            Session.Property("DRIVE_VALID") = "0";
            Session.Property("DRIVE_ERROR_MSG") = "Invalid format '" + raw + "'. Please specify a single drive letter followed by a colon (e.g. V:).";
            return 1;
        }

        var ch = letter.charAt(0);
        if (ch < 'A' || ch > 'Z') {
            Session.Property("DRIVE_VALID") = "0";
            Session.Property("DRIVE_ERROR_MSG") = "'" + ch + "' is not a valid drive letter. Please choose between A: and Z:.";
            return 1;
        }

        if (ch === 'C') {
            Session.Property("DRIVE_VALID") = "0";
            Session.Property("DRIVE_ERROR_MSG") = "Drive letter C: is reserved for the Windows operating system and cannot be used for the storage pool.";
            return 1;
        }

        var fso = new ActiveXObject("Scripting.FileSystemObject");
        if (fso.DriveExists(ch)) {
            Session.Property("DRIVE_VALID") = "0";
            Session.Property("DRIVE_ERROR_MSG") = "Drive letter " + letter + " is already in use by an existing drive, volume, optical disc, or network share. Please select an available drive letter.";
            return 1;
        }

        // Letter is valid and free
        Session.Property("POOL_DRIVE_LETTER") = letter;
        Session.Property("DRIVE_VALID") = "1";
        Session.Property("DRIVE_ERROR_MSG") = "";
    } catch (err) {
        Session.Property("DRIVE_VALID") = "0";
        Session.Property("DRIVE_ERROR_MSG") = "Verification failed: " + err.message;
    }
    return 1;
}

function ApplyConfigDriveLetter() {
    try {
        var installFolder = Session.Property("CustomActionData");
        if (!installFolder || installFolder === "") {
            installFolder = Session.Property("INSTALLFOLDER");
        }

        var targetLetter = Session.Property("POOL_DRIVE_LETTER");
        if (!targetLetter || targetLetter === "") {
            targetLetter = "V:";
        }

        if (!installFolder || installFolder === "") {
            return 1;
        }

        var fso = new ActiveXObject("Scripting.FileSystemObject");
        var configFile = fso.BuildPath(installFolder, "config.json");

        if (fso.FileExists(configFile)) {
            var tsIn = fso.OpenTextFile(configFile, 1, false); // ForReading
            var content = tsIn.ReadAll();
            tsIn.Close();

            // Replace "mount_point": "..."
            var regex = /"mount_point"\s*:\s*"[^"]*"/;
            if (regex.test(content)) {
                var newContent = content.replace(regex, '"mount_point": "' + targetLetter + '"');
                var tsOut = fso.OpenTextFile(configFile, 2, true); // ForWriting
                tsOut.Write(newContent);
                tsOut.Close();
            }
        }
    } catch (e) {
        // Do not block install if file write encounters non-fatal issue
    }
    return 1;
}
