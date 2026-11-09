@file:Suppress("unused")
package io.github.morangsu.mneme.ffi

// uniffi 0.29 生成的是**顶层函数**（见同目录 mneme_ffi.kt）。
// 这里把它们包成 object，让 APP 侧统一以 MnemeApi.xxx() 调用，
// 也避免业务代码依赖生成文件的内部命名。别名 import 防止与 object 方法同名递归。
import io.github.morangsu.mneme.ffi.answer as ffiAnswer
import io.github.morangsu.mneme.ffi.configureSync as ffiConfigureSync
import io.github.morangsu.mneme.ffi.editEntity as ffiEditEntity
import io.github.morangsu.mneme.ffi.entityDetail as ffiEntityDetail
import io.github.morangsu.mneme.ffi.exportData as ffiExportData
import io.github.morangsu.mneme.ffi.importData as ffiImportData
import io.github.morangsu.mneme.ffi.judge as ffiJudge
import io.github.morangsu.mneme.ffi.openDatabase as ffiOpenDatabase
import io.github.morangsu.mneme.ffi.quiz as ffiQuiz
import io.github.morangsu.mneme.ffi.reverse as ffiReverse
import io.github.morangsu.mneme.ffi.review as ffiReview
import io.github.morangsu.mneme.ffi.setSchema as ffiSetSchema
import io.github.morangsu.mneme.ffi.state as ffiState
import io.github.morangsu.mneme.ffi.sync as ffiSync
import io.github.morangsu.mneme.ffi.upstreamAdopt as ffiUpstreamAdopt
import io.github.morangsu.mneme.ffi.upstreamPull as ffiUpstreamPull
import io.github.morangsu.mneme.ffi.upstreamResolve as ffiUpstreamResolve
import io.github.morangsu.mneme.ffi.upstreamStatus as ffiUpstreamStatus
import io.github.morangsu.mneme.ffi.view as ffiView
import io.github.morangsu.mneme.ffi.views as ffiViews

/**
 * Rust 内核的 Kotlin 门面。所有方法返回 JSON 字符串，错误也用 {"error": "..."} 表达。
 * 需要先把 libmneme_ffi.so 放到 app/src/main/jniLibs/<abi>/，详见 android/README.md。
 */
object MnemeApi {
    fun openDatabase(path: String, device: String): String = ffiOpenDatabase(path, device)
    fun state(): String = ffiState()
    fun views(): String = ffiViews()
    fun view(id: String): String = ffiView(id)
    fun reverse(cluesJson: String): String = ffiReverse(cluesJson)
    fun quiz(limit: UInt): String = ffiQuiz(limit)
    fun answer(cardId: String): String = ffiAnswer(cardId)
    fun review(cardId: String, grade: UByte): String = ffiReview(cardId, grade)
    fun judge(cardId: String, input: String): String = ffiJudge(cardId, input)
    fun sync(targetId: String, dry: Boolean): String = ffiSync(targetId, dry)
    fun upstreamPull(): String = ffiUpstreamPull()
    fun upstreamResolve(planId: String, decisionsJson: String): String =
        ffiUpstreamResolve(planId, decisionsJson)
    fun upstreamStatus(): String = ffiUpstreamStatus()
    fun upstreamAdopt(): String = ffiUpstreamAdopt()
    fun importData(content: String, filename: String, dry: Boolean): String =
        ffiImportData(content, filename, dry)
    fun exportData(format: String): String = ffiExportData(format)

    /** 安装领域包（TOML 原文），首次建库用。 */
    fun setSchema(toml: String): String = ffiSetSchema(toml)

    /** 写入 sync.json 并重建同步引擎；configJson 为 SyncConfig 的 JSON。 */
    fun configureSync(configJson: String): String = ffiConfigureSync(configJson)

    /** 读单条实体的全部字段（含 immutable 标记）供编辑面板渲染。 */
    fun entityDetail(entityId: String): String = ffiEntityDetail(entityId)

    /** 批量改字段；immutable 字段会被 Rust 侧拒绝。 */
    fun editEntity(entityId: String, fieldsJson: String): String =
        ffiEditEntity(entityId, fieldsJson)
}
