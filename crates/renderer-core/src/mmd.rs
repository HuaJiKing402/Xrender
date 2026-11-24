//! MMD（PMX）材质私有的着色参数。
//!
//! 通用 [`crate::model::MaterialProfile`] 只描述"用哪个着色模型、用什么贴图、
//! 要不要描边"，toon / sphere 这些 MMD 专有概念集中放在这里。
//! 这样换模型格式（OBJ / glTF / 自己的格式）不需要碰内核的材质分发逻辑，
//! 第 3 步把这一块连同 `pmx.rs` / `vmd.rs` 一起搬进独立 crate 即可。

/// PMX 材质的 toon / sphere 参数。
#[derive(Clone, Debug)]
pub struct MmdMaterial {
    /// PMX `toon_reference`：0 = 引用外部 toon 贴图，1 = 引用内置 toon01..10。
    pub toon_reference: u8,
    /// toon 索引；无 toon 时为 -1。
    pub toon_index: i32,
    /// sphere（环境球）贴图索引。
    pub sphere_index: Option<usize>,
    /// sphere 混合方式：0 关闭、1 乘算、2 加算。
    pub sphere_mode: u8,
    /// PMX 材质绘制标志位（0x01 双面、0x10 描边等），保留给后续阶段使用。
    pub flags: u8,
}

impl MmdMaterial {
    /// 无 toon、无 sphere 的默认形态。
    pub fn neutral() -> Self {
        Self {
            toon_reference: 0,
            toon_index: -1,
            sphere_index: None,
            sphere_mode: 0,
            flags: 0,
        }
    }
}
