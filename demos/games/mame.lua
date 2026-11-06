local directory = assert(os.getenv('RUI_GAME_STATE'))
local subscriptions, lines, controls = {}, {}, {}
local sequence, input_revision, input_mask = 0, 0, 0
local fields = { 'P1 Button 1', 'P1 Button 2', 'P1 Button 3', 'P1 Button 4', 'P1 Button 5', 'Coin 1', '1 Player Start' }
if os.getenv('RUI_GAME') == 'bzone' then
    fields = { 'P1 Left Stick/Up', 'P1 Left Stick/Down', 'P1 Button 1', 'P1 Right Stick/Up', 'P1 Right Stick/Down', 'Coin 1', '1 Player Start' }
end
local function linear(v)
    if v <= 0.04045 then return v / 12.92 end
    return ((v + 0.055) / 1.055) ^ 2.4
end
local function inputs()
    local file = io.open(directory .. '/inputs', 'r')
    local expiry, mask, revision = 0, 0, 0
    if file then
        expiry, mask, revision = file:read('*n', '*n', '*n')
        file:close()
    end
    if not expiry or expiry < os.time() then mask = 0 end
    for i, field in ipairs(controls) do field:set_value((mask >> (i - 1)) & 1) end
    input_revision, input_mask = revision or 0, mask
end
for _, name in ipairs(fields) do
    local found
    for _, port in pairs(manager.machine.ioport.ports) do
        for _, field in pairs(port.fields) do
            if field.name == name then found = field end
        end
    end
    assert(found, 'Missing MAME input: ' .. name)
    controls[#controls + 1] = found
end
local device = manager.machine.vector_devices:at(1)
assert(device, 'MAME vector-device callbacks required')
subscriptions[1] = device:add_frame_begin_notifier(function() lines = {} end)
subscriptions[2] = device:add_line_notifier(function(x0, y0, x1, y1, color, intensity, width, height)
    assert(#lines < 2048, 'RUI vector line ceiling')
    local r = linear(((color >> 16) & 255) / 255 * intensity / 255)
    local g = linear(((color >> 8) & 255) / 255 * intensity / 255)
    local b = linear((color & 255) / 255 * intensity / 255)
    if os.getenv('RUI_GAME') == 'bzone' then r, b = r * 0.25, b * 0.4 end
    lines[#lines + 1] = string.format('[%.4f,%.4f,%.4f,%.4f,%.6f,%.6f,%.6f]',
        x0 / width * 640, y0 / height * 480,
        x1 / width * 640, y1 / height * 480, r, g, b)
end)
subscriptions[3] = device:add_frame_end_notifier(function()
    inputs()
    sequence = sequence + 1
    local file = assert(io.open(directory .. '/frame.tmp', 'w'))
    file:write('{"sequence":', sequence, ',"inputRevision":', input_revision, ',"mask":', input_mask, ',"lines":[', table.concat(lines, ','), ']}')
    file:close()
    assert(os.rename(directory .. '/frame.tmp', directory .. '/frame.json'))
end)
-- Keep subscriptions reachable for the lifetime of this script.
_G.rui_vector_subscriptions = subscriptions
