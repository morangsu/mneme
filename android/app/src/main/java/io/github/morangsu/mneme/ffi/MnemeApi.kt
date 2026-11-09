@file:Suppress("unused")
package io.github.morangsu.mneme.ffi

// uniffi 0.29 生成的是**顶层函数**（见同目录 mneme_ffi.kt）。
// 这里把它们包成 object，让 APP 侧统一以 MnemeApi.xxx() 调用，
// 也避免业务代码依赖生成文件的内部命名。别名 import 防止与 object 方法同名递归。
import io.github.morangsu.mneme.ffi.answer as ffiAnswer
import io.github.morangsu.mneme.ffi.checkDuplicate as ffiCheckDuplicate
import io.github.morangsu.mneme.ffi.configureSync as ffiConfigureSync
import io.github.morangsu.mneme.ffi.createEntities as ffiCreateEntities
import io.github.morangsu.mneme.ffi.createEntity as ffiCreateEntity
import io.github.morangsu.mneme.ffi.deleteEntity as ffiDeleteEntity
import io.github.morangsu.mneme.ffi.deleteGroup as ffiDeleteGroup
import io.github.morangsu.mneme.ffi.entityHistory as ffiEntityHistory
import io.github.morangsu.mneme.ffi.entityTemplate as ffiEntityTemplate
import io.github.morangsu.mneme.ffi.listGroups as ffiListGroups
import io.github.morangsu.mneme.ffi.listDeleted as ffiListDeleted
import io.github.morangsu.mneme.ffi.parsePaste as ffiParsePaste
import io.github.morangsu.mneme.ffi.parsePasteMany as ffiParsePasteMany
import io.github.morangsu.mneme.ffi.previewImport as ffiPreviewImport
import io.github.morangsu.mneme.ffi.restoreEntity as ffiRestoreEntity
import io.github.morangsu.mneme.ffi.setEntityGroup as ffiSetEntityGroup
import io.github.morangsu.mneme.ffi.undoEdit as ffiUndoEdit
import io.github.morangsu.mneme.ffi.upsertGroup as ffiUpsertGroup
import io.github.morangsu.mneme.ffi.editEntity as ffiEditEntity
import io.github.morangsu.mneme.ffi.entityDetail as ffiEntityDetail
import io.github.morangsu.mneme.ffi.exportData as ffiExportData
import io.github.morangsu.mneme.ffi.importData as ffiImportData
import io.github.morangsu.mneme.ffi.judge as ffiJudge
import io.github.morangsu.mneme.ffi.openDatabase as ffiOpenDatabase
import io.github.morangsu.mneme.ffi.quiz as ffiQuiz
import io.github.morangsu.mneme.ffi.reverse as ffiReverse
import io.github.morangsu.mneme.ffi.quizPlan as ffiQuizPlan
import io.github.morangsu.mneme.ffi.quizQueue as ffiQuizQueue
import io.github.morangsu.mneme.ffi.reviewConfig as ffiReviewConfig
import io.github.morangsu.mneme.ffi.setReviewConfig as ffiSetReviewConfig
import io.github.morangsu.mneme.ffi.review as ffiReview
import io.github.morangsu.mneme.ffi.suggestGrade as ffiSuggestGrade
import io.github.morangsu.mneme.ffi.weakCards as ffiWeakCards
import io.github.morangsu.mneme.ffi.setSchema as ffiSetSchema
import io.github.morangsu.mneme.ffi.state as ffiState
import io.github.morangsu.mneme.ffi.sync as ffiSync
import io.github.morangsu.mneme.ffi.upstreamAdopt as ffiUpstreamAdopt
import io.github.morangsu.mneme.ffi.upstreamPull as ffiUpstreamPull
import io.github.morangsu.mneme.ffi.upstreamResolve as ffiUpstreamResolve
import io.github.morangsu.mneme.ffi.upstreamStatus as ffiUpstreamStatus
import io.github.morangsu.mneme.ffi.view as ffiView
import io.github.morangsu.mneme.ffi.views as ffiViews
import io.github.morangsu.mneme.ffi.search as ffiSearch
import io.github.morangsu.mneme.ffi.setCredential as ffiSetCredential
import io.github.morangsu.mneme.ffi.clearCredential as ffiClearCredential
import io.github.morangsu.mneme.ffi.credentials as ffiCredentials
import io.github.morangsu.mneme.ffi.reloadSync as ffiReloadSync

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

    /** 新建空白表单的字段定义。 */
    fun entityTemplate(kind: String): String = ffiEntityTemplate(kind)

    /** 新建条目（immutable 字段允许填，这是"抄错了另建一条"的正路）。 */
    fun createEntity(kind: String, fieldsJson: String, idHint: String): String =
        ffiCreateEntity(kind, fieldsJson, idHint)

    fun deleteEntity(entityId: String): String = ffiDeleteEntity(entityId)
    fun restoreEntity(entityId: String): String = ffiRestoreEntity(entityId)
    fun listDeleted(): String = ffiListDeleted()
    fun entityHistory(entityId: String, limit: UInt): String = ffiEntityHistory(entityId, limit)
    fun undoEdit(entityId: String): String = ffiUndoEdit(entityId)

    /** 把一段中文文本解析成字段；认不出的行原样返回。 */
    fun parsePaste(text: String, kind: String): String = ffiParsePaste(text, kind)

    /** 分组树（含每组成员数）。 */
    fun listGroups(): String = ffiListGroups()

    /** 新建/重命名/改上级；parent 传空串表示顶层。 */
    fun upsertGroup(id: String, name: String, parent: String): String =
        ffiUpsertGroup(id, name, parent)

    fun deleteGroup(groupId: String): String = ffiDeleteGroup(groupId)

    /** 把实体挂到分组；groupId 传空串表示移出分组。 */
    fun setEntityGroup(entityId: String, groupId: String): String =
        ffiSetEntityGroup(entityId, groupId)

    /** 导入预览：逐条差异，只读。 */
    fun previewImport(content: String, filename: String): String =
        ffiPreviewImport(content, filename)

    /** 批量粘贴解析（空行分隔多条）。 */
    fun parsePasteMany(text: String, kind: String): String = ffiParsePasteMany(text, kind)

    /** 批量新建：逐条独立，返回 created/skipped/failed。 */
    fun createEntities(kind: String, itemsJson: String): String =
        ffiCreateEntities(kind, itemsJson)

    /** 查重：同名 / 同义名 / 近似名；excludeId 传自己时不会「和自己重复」。 */
    fun checkDuplicate(kind: String, fieldsJson: String, excludeId: String): String =
        ffiCheckDuplicate(kind, fieldsJson, excludeId)

    /** 每日队列：到期优先、新卡限量。 */
    fun quizQueue(newLimit: UInt, dueLimit: UInt): String = ffiQuizQueue(newLimit, dueLimit)

    /** 自动评分建议：对错 + 相似度 + 用时 → 1..4。 */
    fun suggestGrade(correct: Boolean, elapsedMs: Long, score: Double): String =
        ffiSuggestGrade(correct, elapsedMs, score)

    /** 错题本：最近 30 天末尾连错 >=2 次。 */
    fun weakCards(limit: UInt): String = ffiWeakCards(limit)

    /** 按用户配置（review.json）出题，含本轮张数限制。 */
    fun quizPlan(): String = ffiQuizPlan()

    fun reviewConfig(): String = ffiReviewConfig()

    fun setReviewConfig(configJson: String): String = ffiSetReviewConfig(configJson)

    /** 全文检索：跨字段 + 同义名 + 拼音首字母（mh → 麻黄）；limit 传 0 按 50 条截断。 */
    fun search(query: String, limit: UInt): String = ffiSearch(query, limit)

    /** 注入凭据到进程内存（不落盘）；宿主负责用 Keystore 持久化。 */
    fun setCredential(name: String, secret: String): String = ffiSetCredential(name, secret)

    fun clearCredential(name: String): String = ffiClearCredential(name)

    /** 已注入的凭据名（不含值）。 */
    fun credentials(): String = ffiCredentials()

    /** 凭据注入后重建同步引擎（不写盘）。 */
    fun reloadSync(): String = ffiReloadSync()
}
