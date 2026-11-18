-- Event-driven instance progress. Saved on logout/reload; no combat-log handler.
local frame = CreateFrame("Frame")
local session, instance, instanceDifficulty, total, complete
local kills = {}
local lastRequest = 0
local lastIdentity, lastIdentityTime
local function Hex(value)
    return (tostring(value or ""):gsub(".", function(c) return string.format("%02x", string.byte(c)) end))
end
local function SaveIdentity()
    if not session or not GetNumSavedInstances or not GetSavedInstanceInfo then return end
    local name, kind, currentDifficulty = GetInstanceInfo()
    if kind ~= "raid" then return end
    for i = 1, GetNumSavedInstances() do
        local savedName, id, reset, difficulty, locked, _, high, raid = GetSavedInstanceInfo(i)
        if savedName == name and difficulty == currentDifficulty and locked and raid and id and reset and reset > 0 then
            RaidAutoLogDB = RaidAutoLogDB or {enabled=true}
            local identity = tostring(id) .. ":" .. tostring(high or 0) .. ":" .. tostring(difficulty)
            if lastIdentity and identity ~= lastIdentity then session = time() end
            if identity == lastIdentity and time() - (lastIdentityTime or 0) < 30 then return end
            lastIdentity, lastIdentityTime = identity, time()
            local rows = RaidAutoLogDB.lockouts or {}
            local keep = {}
            for _, row in ipairs(rows) do
                local stamp = tonumber(row:match("^RAL2|(%d+)|"))
                if stamp and stamp > time() - 30*86400 then keep[#keep+1] = row end
            end
            keep[#keep+1] = table.concat({"RAL2",time(),Hex((GetCVar and GetCVar("realmList")) or GetRealmName()),
                Hex(UnitName("player")),Hex(name),Hex(string.format("%.0f",id)),Hex(tostring(high or 0)),
                difficulty,math.floor(time()+reset),session},"|")
            while #keep > 1000 do table.remove(keep,1) end
            RaidAutoLogDB.lockouts = keep
        end
    end
end
local function Save()
    if not session or not total or not complete then return end
    RaidAutoLogDB = RaidAutoLogDB or {enabled = true}
    local rows = RaidAutoLogDB.progress or {}
    RaidAutoLogDB.progress = rows
    local now = time()
    RaidAutoLogDB.progressActive = {instance=instance, session=session, total=total, complete=complete, time=now}
    local keep = {}
    for _, row in ipairs(rows) do
        local stamp = tonumber(row:match("^RAL1|(%d+)|"))
        if stamp and stamp > now - 30*86400 then keep[#keep+1] = row end
    end
    keep[#keep+1] = table.concat({"RAL1", now, Hex((GetCVar and GetCVar("realmList")) or GetRealmName()), Hex(UnitName("player")),
        Hex(instance), total, math.min(complete, total), session}, "|")
    while #keep > 500 do table.remove(keep, 1) end
    RaidAutoLogDB.progress = keep
end
local function Enter()
    local inside, kind = IsInInstance()
    if not inside or kind ~= "raid" then
        if RaidAutoLogDB then RaidAutoLogDB.progressActive = nil end
        session, instance, total, complete = nil, nil, nil, nil
        instanceDifficulty, lastIdentity, lastIdentityTime = nil, nil, nil
        kills = {}
        return
    end
    local name, _, difficulty = GetInstanceInfo()
    if instance ~= name or difficulty ~= instanceDifficulty then
        instance, session, total, complete = name, time(), nil, nil
        instanceDifficulty = difficulty
        local old = RaidAutoLogDB and RaidAutoLogDB.progressActive
        if old and old.instance == name and time() - old.time < 3600 then
            session, total, complete = old.session, old.total, old.complete
        end
        kills = {}
    end
    if RequestRaidInfo and time() - lastRequest >= 5 then
        lastRequest = time()
        RequestRaidInfo()
    end
end
local function LockProgress()
    Enter()
    if not session or not GetInstanceLockTimeRemaining then return end
    local _, _, count, dead = GetInstanceLockTimeRemaining()
    if not count and GetInstanceLockTimeRemainingEncounter then
        local found, defeated = 0, 0
        for i = 1, 50 do
            local ok, boss, _, killed = pcall(GetInstanceLockTimeRemainingEncounter, i)
            if not ok or not boss then break end
            found = found + 1
            if killed then defeated = defeated + 1 end
        end
        if found > 0 then count, dead = found, defeated end
    end
    if type(count) == "number" and type(dead) == "number" and count > 0 then
        total, complete = count, dead
        kills = {}
        Save()
    end
end
local installed = false
local function SavedProgress()
    if not session or not GetNumSavedInstances then return end
    for i = 1, GetNumSavedInstances() do
        local name, _, _, _, locked, _, _, raid, _, _, count, dead = GetSavedInstanceInfo(i)
        if name == instance and locked and raid and type(count) == "number" and type(dead) == "number" and count > 0 then
            total, complete = count, dead
            kills = {}
            Save()
        end
    end
end
local function Install()
    if installed or not DBM or not DBM.RegisterCallback then return end
    DBM:RegisterCallback("DBM_Kill", function(_, mod)
        SaveIdentity()
        if RequestRaidInfo then RequestRaidInfo() end
        if not session or not mod or mod.isTrashMod or kills[mod] then return end
        kills[mod] = true
        if complete then complete = complete + 1; Save() end
    end)
    installed = true
end
frame:SetScript("OnEvent", function(_, event)
    Install()
    if event == "INSTANCE_LOCK_START" then LockProgress()
    elseif event == "UPDATE_INSTANCE_INFO" then SavedProgress()
    elseif event == "PLAYER_LOGOUT" then Save()
    else Enter() end
    SaveIdentity()
end)
frame:RegisterEvent("PLAYER_LOGIN")
frame:RegisterEvent("PLAYER_ENTERING_WORLD")
frame:RegisterEvent("ZONE_CHANGED_NEW_AREA")
frame:RegisterEvent("INSTANCE_LOCK_START")
frame:RegisterEvent("UPDATE_INSTANCE_INFO")
frame:RegisterEvent("ADDON_LOADED")
frame:RegisterEvent("PLAYER_LOGOUT")
