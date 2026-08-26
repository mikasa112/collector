-- 公共工具模块，供其他脚本通过 require("_utils") 引入。
-- 文件名以 _ 开头，不会被引擎当作独立脚本加载/热更新。

local M = {}

--- 通用过滤函数
---@param points DataPoint[]
---@param predicate fun(value: DataPoint, index: integer): boolean
---@return DataPoint[]
function M.filter(points, predicate)
    local result = {}
    for i, value in ipairs(points) do
        if predicate(value, i) then
            table.insert(result, value)
        end
    end

    return result
end

--- 依次拼接多组数据点，并将 id 重新编号为从 start_id 开始累加的连续序号，
--- 避免各设备原始 id 各自从小数字起步、合并后互相冲突。
---@param segments DataPoint[][] 按拼接顺序排列的多组数据点
---@param start_id integer? 编号起始值，缺省为 1
---@return DataPoint[]
function M.merge_with_id(segments, start_id)
    local result = {}
    local next_id = start_id or 1
    for _, seg in ipairs(segments) do
        for _, point in ipairs(seg) do
            local merged = {}
            for k, v in pairs(point) do
                merged[k] = v
            end
            merged.id = next_id
            table.insert(result, merged)
            next_id = next_id + 1
        end
    end

    return result
end

--- 给一组数据点标记来源设备与原始 id，供合并编号后仍能反查回原始点位。
--- 需在 merge_with_id 重新编号之前调用，否则原始 id 会被覆盖丢失。
---@param points DataPoint[]
---@param dev_id string
---@return DataPoint[]
function M.tag_dev(points, dev_id)
    local result = {}
    for i, p in ipairs(points) do
        local tagged = {}
        for k, v in pairs(p) do
            tagged[k] = v
        end
        tagged.dev = dev_id
        tagged.orig_id = p.id
        result[i] = tagged
    end
    return result
end

--- 依据合并编号后的数据点构建“虚拟id -> {dev, id}”反查表，
--- 用于遥调/遥控下行指令按虚拟id映射回原始设备点位后再 dc.dispatch。
--- 数据点须已经过 tag_dev 标记，否则无 dev/orig_id 的点会被跳过。
---@param points DataPoint[]
---@return table<integer, {dev: string, id: integer}>
function M.build_map(points)
    local map = {}
    for _, p in ipairs(points) do
        if p.dev ~= nil and p.orig_id ~= nil then
            map[p.id] = { dev = p.dev, id = p.orig_id }
        end
    end
    return map
end

return M
