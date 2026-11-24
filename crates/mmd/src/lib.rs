//! MMD 资源层：PMX 模型与 VMD 动作的解析。
//!
//! 依赖方向是单向的：`mmd` 依赖 `renderer-core`，内核不认识 MMD 文件格式。
//! 因此换模型格式只需要在这里加一个模块，内核的渲染与管线代码不动。

pub mod pmx;
pub mod vmd;

pub use pmx::{load_pmx, load_pmx_from_path};
pub use vmd::parse_vmd;
