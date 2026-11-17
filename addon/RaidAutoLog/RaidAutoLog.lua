local frame = CreateFrame("Frame")
local activeBosses = {}
local callbacksInstalled = false
local function Message(text)
    DEFAULT_CHAT_FRAME:AddMessage("|cff66ccffRaidAutoLog:|r " .. text)
end
local function InRaid()
    local inside, kind = IsInInstance()
    return inside and kind == "raid"
end
local function StopRecording()
    if RaidAutoLogDB and RaidAutoLogDB.owned then
        if LoggingCombat() then LoggingCombat(false) end
        RaidAutoLogDB.owned = nil
        Message("Boss recording stopped.")
    end
end
local function StartBoss(event, mod)
    if not RaidAutoLogDB or RaidAutoLogDB.enabled == false or not InRaid() then return end
    if mod and mod.isTrashMod then return end
    activeBosses.recovery = nil
    activeBosses[mod or "boss"] = true
    if not LoggingCombat() then LoggingCombat(true) end
    RaidAutoLogDB.owned = true
    Message("Recording boss encounter.")
end
local function EndBoss(event, mod)
    activeBosses.recovery = nil
    activeBosses[mod or "boss"] = nil
    if not next(activeBosses) then StopRecording() end
end
local function InstallCallbacks()
    if callbacksInstalled or not DBM or not DBM.RegisterCallback then return end
    DBM:RegisterCallback("DBM_Pull", StartBoss)
    DBM:RegisterCallback("DBM_Kill", EndBoss)
    DBM:RegisterCallback("DBM_Wipe", EndBoss)
    callbacksInstalled = true
end
local function RestoreState()
    InstallCallbacks()
    activeBosses = {}
    if RaidAutoLogDB and RaidAutoLogDB.enabled ~= false and InRaid()
        and DBM and DBM.IsEncounterInProgress and DBM:IsEncounterInProgress() then
        activeBosses.recovery = true
        if not LoggingCombat() then LoggingCombat(true) end
        RaidAutoLogDB.owned = true
    else
        StopRecording()
    end
end
frame:SetScript("OnEvent", function(self, event, addon)
    if event == "ADDON_LOADED" then
        if addon == "RaidAutoLog" then
            RaidAutoLogDB = RaidAutoLogDB or { enabled = true }
            RestoreState()
        elseif addon == "DBM-Core" then
            InstallCallbacks()
        end
    else
        RestoreState()
        if event == "PLAYER_LOGIN" and not callbacksInstalled then
            Message("DBM-Core is required for boss-only recording. Recording is OFF.")
        end
    end
end)
frame:RegisterEvent("ADDON_LOADED")
frame:RegisterEvent("PLAYER_LOGIN")
frame:RegisterEvent("PLAYER_ENTERING_WORLD")
frame:RegisterEvent("ZONE_CHANGED_NEW_AREA")
SLASH_RAIDAUTOLOG1 = "/raidlog"
SlashCmdList.RAIDAUTOLOG = function(command)
    command = string.lower(command or "")
    if command == "auto" then
        RaidAutoLogDB.enabled = true
        RestoreState()
        Message("Automatic boss-only recording enabled (DBM).")
    elseif command == "off" then
        RaidAutoLogDB.enabled = false
        activeBosses = {}
        StopRecording()
        LoggingCombat(false)
        Message("Recording disabled. Use /raidlog auto to enable boss-only mode.")
    else
        Message("Boss-only: " .. (RaidAutoLogDB.enabled == false and "OFF" or "ON")
            .. "; DBM: " .. (callbacksInstalled and "OK" or "MISSING")
            .. "; recording: " .. (LoggingCombat() and "ON" or "OFF"))
    end
end
