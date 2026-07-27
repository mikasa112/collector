use std::time::Duration;

use collector_core::{
    center::SharedPointCenter,
    core::point::{DownDataPoint, PointId},
    utils::taos::{TaosDbError, get_taos},
};
use taos::AsyncQueryable;

use crate::{
    DataDriven,
    strategy::{Schedule, Strategy, StrategyError},
};

/// pcs_data 列定义（与建表语句列顺序严格一致）: (点位ID, 列名, 中文含义)
const PCS_COLUMNS: &[(PointId, &str, &str)] = &[
    (1, "pcs_va", "PCS端口A相电压"),
    (2, "pcs_vb", "PCS端口B相电压"),
    (3, "pcs_vc", "PCS端口C相电压"),
    (4, "pcs_ia", "PCS端口A相电流"),
    (5, "pcs_ib", "PCS输出B相电流"),
    (6, "pcs_ic", "PCS输出C相电流"),
    (7, "grid_freq", "电网频率"),
    (8, "pcs_pa", "PCS A相输出有功功率"),
    (9, "pcs_pb", "PCS B相输出有功功率"),
    (10, "pcs_pc", "PCS C相输出有功功率"),
    (11, "pcs_p_total", "PCS总输出有功功率"),
    (12, "pcs_qa", "PCS A相输出无功功率"),
    (13, "pcs_qb", "PCS B相输出无功功率"),
    (14, "pcs_qc", "PCS C相输出无功功率"),
    (15, "pcs_q_total", "PCS总输出无功功率"),
    (16, "pcs_sa", "PCS A相输出视在功率"),
    (17, "pcs_sb", "PCS B相输出视在功率"),
    (18, "pcs_sc", "PCS C相输出视在功率"),
    (19, "pcs_s_total", "PCS总输出视在功率"),
    (20, "pcs_pf_a", "PCS输出A相功率因数"),
    (21, "pcs_pf_b", "PCS输出B相功率因数"),
    (22, "pcs_pf_c", "PCS输出C相功率因数"),
    (23, "pcs_pf_total", "PCS输出总功率因数"),
    (24, "pcs_input_power", "PCS输入功率"),
    (25, "pcs_input_voltage", "PCS输入电压"),
    (26, "pcs_input_current", "PCS输入电流"),
    (30, "pcs_ac_charge_energy", "PCS交流累计充电电量"),
    (31, "pcs_ac_discharge_energy", "PCS交流累计放电电量"),
    (32, "pcs_dc_charge_energy", "PCS直流累计充电电量"),
    (33, "pcs_dc_discharge_energy", "PCS直流累计放电电量"),
];

/// bcu_data 列定义（与建表语句列顺序严格一致）: (点位ID, 列名, 中文含义)
const BCU_COLUMNS: &[(PointId, &str, &str)] = &[
    (5, "run_status", "运行状态"),
    (6, "cell_sum_voltage", "单体累加和总压"),
    (7, "voltage_diff", "采集总压与累加总压差"),
    (8, "precharge_voltage", "预充总压"),
    (9, "cell_max_voltage", "单体最高电压"),
    (10, "cell_max_voltage_no", "单体最高电压编号"),
    (11, "cell_max_slave_no", "最高单体所在从控号"),
    (12, "cell_max_slave_cell_no", "最高单体在从控中的单体编号"),
    (13, "cell_min_voltage", "单体最低电压"),
    (14, "cell_min_voltage_no", "单体最低电压编号"),
    (15, "cell_min_slave_no", "最低单体所在从控编号"),
    (16, "cell_min_slave_cell_no", "最低单体在从控中的单体编号"),
    (17, "cell_voltage_diff", "最高最低单体压差"),
    (18, "cell_avg_voltage", "单体平均电压"),
    (19, "temp_max", "电池最高温度"),
    (20, "temp_max_no", "电池最高温度编号"),
    (21, "temp_max_slave_no", "最高温度所在从控编号"),
    (22, "temp_max_slave_temp_no", "最高温度在从控中的温度编号"),
    (23, "temp_min", "电池最低温度"),
    (24, "temp_min_no", "电池最低温度编号"),
    (25, "temp_min_slave_no", "最低温度所在从控编号"),
    (26, "temp_min_slave_temp_no", "最低温度在从控中的温度编号"),
    (27, "temp_avg", "平均温度"),
    (28, "pole_temp_max", "最大极柱温度"),
    (29, "pole_temp_max_no", "最大极柱温度编号"),
    (30, "max_charge_current", "最大可接受充电电流"),
    (31, "max_discharge_current", "最大可接受放电电流"),
    (32, "soc", "显示SOC"),
    (33, "soh", "SOH"),
    (34, "charge_discharge_status", "充放电状态"),
    (35, "full_empty_flag", "充满放空标志"),
    (36, "chargeable_capacity", "可充电量"),
    (37, "dischargeable_capacity", "可放电量"),
    (38, "last_charge_capacity", "最近一次充电量"),
    (39, "last_discharge_capacity", "最近一次放电量"),
    (40, "total_charge_energy_high", "累计充电电量高16位"),
    (41, "total_charge_energy_low", "累计充电电量低16位"),
    (42, "total_discharge_energy_high", "累计放电电量高16位"),
    (43, "total_discharge_energy_low", "累计放电电量低16位"),
    (44, "total_voltage", "电池总电压"),
    (45, "load_voltage", "负载电压"),
    (46, "total_current", "总电流"),
];

/// tms_data 列定义（与建表语句列顺序严格一致）: (点位ID, 列名, 中文含义)
const TMS_COLUMNS: &[(PointId, &str, &str)] = &[
    (1, "ambient_temp", "环境温度"),
    (2, "return_liquid_temp", "回液温度"),
    (3, "supply_liquid_temp", "供液温度"),
    (4, "return_liquid_pressure", "回液压力"),
    (5, "supply_liquid_pressure", "供液压力"),
];

pub(crate) struct TaosWriter {
    center: SharedPointCenter,
}

impl TaosWriter {
    pub fn new(center: SharedPointCenter) -> Self {
        Self { center }
    }

    /// 拼出 `CREATE TABLE IF NOT EXISTS emu.<table> (ts TIMESTAMP, col1 DOUBLE, ...)`
    fn create_table_sql(table: &str, columns: &[(PointId, &str, &str)]) -> String {
        let mut sql = format!("CREATE TABLE IF NOT EXISTS emu.{table} (ts TIMESTAMP");
        for (_, col, _) in columns {
            sql.push_str(&format!(", {col} DOUBLE"));
        }
        sql.push(')');
        sql
    }

    /// 按列顺序逐点读取（而非 read_many，避免缺失点位导致的数量错位），
    /// 缺失点位记为 NULL，拼出 INSERT 语句
    fn insert_sql(&self, dev: &str, table: &str, columns: &[(PointId, &str, &str)]) -> String {
        let values = columns
            .iter()
            .map(|(id, _, _)| {
                self.center
                    .read(dev, *id)
                    .and_then(|p| p.value.as_f64().ok())
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "NULL".to_string())
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("INSERT INTO emu.{table} VALUES (now, {values})")
    }
}

#[async_trait::async_trait]
impl Strategy for TaosWriter {
    fn name(&self) -> &str {
        "taos数据写入"
    }

    fn schedule(&self) -> Schedule {
        Schedule::Interval(Duration::from_secs(1))
    }

    async fn on_start(&mut self) -> Result<(), StrategyError> {
        let taos = get_taos()?;
        // KEEP 90：数据仅保留约3个月，超期由 TDengine 自动清理（仅在库首次创建时生效，
        // 若库已存在需用 ALTER DATABASE emu KEEP 90 手动调整）
        taos.exec("CREATE DATABASE IF NOT EXISTS emu KEEP 90")
            .await
            .map_err(TaosDbError::TaosError)?;
        taos.exec(Self::create_table_sql("pcs_data", PCS_COLUMNS))
            .await
            .map_err(TaosDbError::TaosError)?;
        taos.exec(Self::create_table_sql("bcu_data", BCU_COLUMNS))
            .await
            .map_err(TaosDbError::TaosError)?;
        taos.exec(Self::create_table_sql("tms_data", TMS_COLUMNS))
            .await
            .map_err(TaosDbError::TaosError)?;
        Ok(())
    }

    async fn on_tick(&mut self) -> Result<(), StrategyError> {
        let taos = get_taos()?;
        taos.exec(self.insert_sql("pcs", "pcs_data", PCS_COLUMNS))
            .await
            .map_err(TaosDbError::TaosError)?;
        taos.exec(self.insert_sql("bcu", "bcu_data", BCU_COLUMNS))
            .await
            .map_err(TaosDbError::TaosError)?;
        taos.exec(self.insert_sql("tms", "tms_data", TMS_COLUMNS))
            .await
            .map_err(TaosDbError::TaosError)?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl DataDriven for TaosWriter {
    async fn down(&self, _points: &[DownDataPoint]) -> Result<(), StrategyError> {
        Ok(())
    }
}
