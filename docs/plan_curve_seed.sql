-- 计划曲线示例数据：全年、周一到周五生效，周六周日无效，正充负放
-- 00:00-4:00  充电 50kW， soc_limit 95
-- 10:00-12:00 放电 100kW，soc_limit 5
-- 12:00-16:00 充电 50kW， soc_limit 95
-- 18:00-20:00 放电 100kW，soc_limit 5
-- 其余时段，功率清零，无 soc 限制

-- 主表
INSERT INTO t_plan_curve_master
    (curve_name, curve_type, priority, status, valid_start_date, valid_end_date, effective_weekdays, created_by, remark)
VALUES
    ('工作日峰谷曲线', 2, 1, 1, '2026-01-01', '2026-12-31', '1,2,3,4,5', 'system',
     '仅周一到周五生效：0-4点充电50kW，10-12点放电100kW，12-16点充电50kW，18-20点放电100kW，其余时段清零');

-- 明细：0:00-4:00 功率50，soc_limit 95 (time_index 0-15)
WITH RECURSIVE seq(time_index) AS (
    SELECT 0
    UNION ALL
    SELECT time_index + 1 FROM seq WHERE time_index < 15
)
INSERT INTO t_plan_curve_detail (curve_id, time_index, power_value, soc_limit)
SELECT (SELECT id FROM t_plan_curve_master WHERE curve_name = '工作日峰谷曲线' ORDER BY id DESC LIMIT 1), time_index, 50, 95
FROM seq;

-- 明细：4:00-10:00 功率0，无soc限制 (time_index 16-39)
WITH RECURSIVE seq(time_index) AS (
    SELECT 16
    UNION ALL
    SELECT time_index + 1 FROM seq WHERE time_index < 39
)
INSERT INTO t_plan_curve_detail (curve_id, time_index, power_value, soc_limit)
SELECT (SELECT id FROM t_plan_curve_master WHERE curve_name = '工作日峰谷曲线' ORDER BY id DESC LIMIT 1), time_index, 0, NULL
FROM seq;

-- 明细：10:00-12:00 功率-100，soc_limit 5 (time_index 40-47)
WITH RECURSIVE seq(time_index) AS (
    SELECT 40
    UNION ALL
    SELECT time_index + 1 FROM seq WHERE time_index < 47
)
INSERT INTO t_plan_curve_detail (curve_id, time_index, power_value, soc_limit)
SELECT (SELECT id FROM t_plan_curve_master WHERE curve_name = '工作日峰谷曲线' ORDER BY id DESC LIMIT 1), time_index, -100, 5
FROM seq;

-- 明细：12:00-16:00 功率50，soc_limit 95 (time_index 48-63)
WITH RECURSIVE seq(time_index) AS (
    SELECT 48
    UNION ALL
    SELECT time_index + 1 FROM seq WHERE time_index < 63
)
INSERT INTO t_plan_curve_detail (curve_id, time_index, power_value, soc_limit)
SELECT (SELECT id FROM t_plan_curve_master WHERE curve_name = '工作日峰谷曲线' ORDER BY id DESC LIMIT 1), time_index, 50, 95
FROM seq;

-- 明细：16:00-18:00 功率0，无soc限制 (time_index 64-71)
WITH RECURSIVE seq(time_index) AS (
    SELECT 64
    UNION ALL
    SELECT time_index + 1 FROM seq WHERE time_index < 71
)
INSERT INTO t_plan_curve_detail (curve_id, time_index, power_value, soc_limit)
SELECT (SELECT id FROM t_plan_curve_master WHERE curve_name = '工作日峰谷曲线' ORDER BY id DESC LIMIT 1), time_index, 0, NULL
FROM seq;

-- 明细：18:00-20:00 功率-100，soc_limit 5 (time_index 72-79)
WITH RECURSIVE seq(time_index) AS (
    SELECT 72
    UNION ALL
    SELECT time_index + 1 FROM seq WHERE time_index < 79
)
INSERT INTO t_plan_curve_detail (curve_id, time_index, power_value, soc_limit)
SELECT (SELECT id FROM t_plan_curve_master WHERE curve_name = '工作日峰谷曲线' ORDER BY id DESC LIMIT 1), time_index, -100, 5
FROM seq;

-- 明细：20:00-24:00 功率0，无soc限制 (time_index 80-95)
WITH RECURSIVE seq(time_index) AS (
    SELECT 80
    UNION ALL
    SELECT time_index + 1 FROM seq WHERE time_index < 95
)
INSERT INTO t_plan_curve_detail (curve_id, time_index, power_value, soc_limit)
SELECT (SELECT id FROM t_plan_curve_master WHERE curve_name = '工作日峰谷曲线' ORDER BY id DESC LIMIT 1), time_index, 0, NULL
FROM seq;
