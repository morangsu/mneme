package io.github.morangsu.mneme

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject

/**
 * Rust ↔ Kotlin 桥接层。
 *
 * 真实绑定由 uniffi 生成，位于 `io.github.morangsu.mneme.ffi`，顶层对象 `MnemeApi`，
 * 所有方法均返回 serde_json 序列化的 JSON 字符串。
 *
 * 真实绑定已由 uniffi 生成并接入：`io.github.morangsu.mneme.ffi.MnemeApi`。
 * `MnemeBridge.call` 默认调用 Rust；只有在 `.so` 尚未放进 jniLibs 时
 * （UnsatisfiedLinkError）才回退到 `MnemeApiStub` 的样例 JSON，保证 UI 仍可编译运行。
 *
 * 生成绑定与放置 .so 的步骤见 android/README.md。
 */
import io.github.morangsu.mneme.ffi.MnemeApi

// ---------------------------------------------------------------------------
// 兜底：本地样例 JSON。结构对齐 crates/mneme-server 与 crates/mneme-upstream 的真实字段。
// ---------------------------------------------------------------------------
object MnemeApiStub {

    fun call(method: String, args: Map<String, Any?>): String = when (method) {
        "openDatabase" -> JSONObject()
            .put("ok", true)
            .put("path", args["path"] ?: "")
            .put("device", args["device"] ?: "")
            .put("rev", 2)
            .toString()

        "state" -> SAMPLE_STATE
        "views" -> SAMPLE_VIEWS
        "view" -> SAMPLE_VIEW
        "reverse" -> SAMPLE_REVERSE
        "quiz" -> SAMPLE_QUIZ
        "quizQueue" -> SAMPLE_QUIZ
        "suggestGrade" -> JSONObject().put("grade", 3).put("reason", "正常速度").toString()
        "weakCards" -> "{\"items\":[]}"
        "quizPlan" -> SAMPLE_QUIZ
        "reviewConfig" -> "{\"new_per_day\":10,\"reviews_per_day\":100,\"session_limit\":20,\"desired_retention\":0.9,\"max_interval_days\":3650,\"easy_interval_days\":4,\"leech_threshold\":8,\"learning_steps_minutes\":[1,10]}"
        "setReviewConfig" -> "{\"ok\":true}"
        "answer" -> SAMPLE_ANSWER
        "entityDetail" -> SAMPLE_ENTITY_DETAIL
        "entityTemplate" -> SAMPLE_ENTITY_DETAIL
        "createEntity" -> JSONObject().put("ok", true).put("id", "测试药").put("rev", 1).put("name", "测试药").toString()
        "deleteEntity" -> JSONObject().put("ok", true).put("id", args["entityId"] ?: "").put("deleted_at", 0).toString()
        "restoreEntity" -> JSONObject().put("ok", true).put("id", args["entityId"] ?: "").put("rev", 9).toString()
        "listDeleted" -> "{\"items\":[]}"
        "entityHistory" -> "{\"items\":[]}"
        "undoEdit" -> JSONObject().put("ok", true).put("rev", 9).put("restored", JSONArray().put("effects")).toString()
        "editEntity" -> JSONObject()
            .put("ok", true)
            .put("id", args["entityId"] ?: "")
            .put("rev", 99)
            .put("changed", JSONArray().put(JSONObject().put("key", "features").put("before", "发汗力强").put("after", "示例")))
            .toString()

        "upstreamStatus" -> SAMPLE_UPSTREAM_STATUS
        "upstreamPull" -> SAMPLE_PULL_PLAN

        "judge" -> {
            val input = args["input"] as? String ?: ""
            val correct = input.contains("麻黄") || input.contains("发汗解表")
            JSONObject()
                .put("score", if (correct) 0.93 else 0.41)
                .put("correct", correct)
                .put("matched", JSONArray().put(if (correct) "宣肺平喘" else ""))
                .toString()
        }

        "review" -> {
            val cardId = args["cardId"] as? String ?: "?"
            val grade = (args["grade"] as? UByte)?.toInt() ?: 3
            JSONObject()
                .put("ok", true)
                .put("applied", 1)
                .put("grade", grade)
                .put("event", "$cardId:${System.currentTimeMillis()}")
                .toString()
        }

        "sync" -> {
            val dry = args["dry"] as? Boolean ?: false
            val target = args["targetId"] as? String ?: "github-private"
            JSONObject()
                .put("ok", true)
                .put("target", target)
                .put("dry", dry)
                .put(
                    "report",
                    (if (dry) "预演：" else "") +
                        "上行 22182 B / 快照 snap-00000003-a91f0c22.json / 冲突 0 / 重试 0",
                )
                .put("sha256", "a91f0c22d4e55b7c3f0e19d8a6b2c4417e9f0a3d5c8b1e2f4a6c8d0e9f1a2b3")
                .put("bytes", 22182)
                .put("conflicts", 0)
                .put("retries", 0)
                .toString()
        }

        "upstreamResolve" -> JSONObject()
            .put("plan_id", args["planId"] ?: "")
            .put("auto_applied", 5)
            .put("conflicts_resolved", 4)
            .put("added", 1)
            .put("removed", 0)
            .put("keep_mine", 3)
            .put("take_upstream", 1)
            .put("keep_both", 0)
            .put("new_rev", 7)
            .toString()

        "importData" -> {
            val dry = args["dry"] as? Boolean ?: false
            JSONObject()
                .put("ok", true)
                .put("format", "csv")
                .put("imported", 6)
                .put("skipped", 0)
                .put("merged", 1)
                .put("conflicts", 0)
                .put("dry", dry)
                .put("sha256", "1fa7a373b0c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f708192a3b4c5d6e")
                .toString()
        }

        "exportData" -> JSONObject()
            .put("ok", true)
            .put("format", args["format"] ?: "bundle")
            .put("bytes", 13047)
            .put("sha256", "713cb045a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d")
            .put("content", "（示例）format=mneme-bundle/1; entities=6; cards=77 —— 真实内容由 Rust serde_json 序列化返回")
            .toString()

        else -> "{\"error\":\"未知 FFI 方法：$method\"}"
    }

    private const val SAMPLE_STATE =
        """{"schema":"中药学","entities":6,"groups":3,"cards":77,"events":128,"rev":2,"aliases":2,"device":"android-8f3c2a"}"""

    private const val SAMPLE_VIEWS =
        """{"views":[
{"id":"all","label":"全部药物","columns":["name","group_name","effects","nature","dosage"]},
{"id":"danger","label":"危险区 · 原样字段","columns":["name","nature","toxicity","dosage","compatibility"]},
{"id":"reverse","label":"可反查线索","columns":["name","effects","indications","features","contraindications"]}
]}"""

    private const val SAMPLE_VIEW =
        """{
"columns":[
{"key":"name","label":"药名","immutable":false},
{"key":"group_name","label":"药群","immutable":false},
{"key":"effects","label":"功效","immutable":false},
{"key":"nature","label":"性味归经","immutable":true},
{"key":"dosage","label":"用量","immutable":true}
],
"primary":"name",
"hidden":[],
"group_by":"group_name",
"rows":[
{"name":"麻黄","group_name":"发散风寒药","effects":"发汗解表,宣肺平喘,利水消肿","nature":"辛、微苦，温。归肺、膀胱经","dosage":"2–10g","_group":"发散风寒药"},
{"name":"桂枝","group_name":"发散风寒药","effects":"发汗解肌,温通经脉,助阳化气","nature":"辛、甘，温。归心、肺、膀胱经","dosage":"3–10g","_group":"发散风寒药"},
{"name":"紫苏","group_name":"发散风寒药","effects":"解表散寒,行气和胃","nature":"辛，温。归肺、脾经","dosage":"5–10g","_group":"发散风寒药"},
{"name":"石膏","group_name":"清热泻火药","effects":"清热泻火,除烦止渴","nature":"甘、辛，大寒。归肺、胃经","dosage":"15–60g","_group":"清热泻火药"},
{"name":"知母","group_name":"清热泻火药","effects":"清热泻火,滋阴润燥","nature":"苦、甘，寒。归肺、胃、肾经","dosage":"6–12g","_group":"清热泻火药"}
]
}"""

    private const val SAMPLE_REVERSE =
        """{"clues":[["herb.effects","宣肺平喘"],["herb.contraindications","表虚自汗"]],"items":[
{"id":"herb-mahuang","name":"麻黄","group":"发散风寒药","score":1.0,"summary":"effects:发汗解表 / 宣肺平喘 / 利水消肿 · indications:风寒感冒 · features:发汗力强"},
{"id":"herb-xingren","name":"苦杏仁","group":"止咳平喘药","score":0.42,"summary":"effects:降气止咳平喘 / 润肠通便 · indications:咳嗽气喘"}
]}"""

    private const val SAMPLE_QUIZ =
        """{"due":3,"cards":[
{"id":"card-mahuang-effects","prompt":"功效：发汗解表、宣肺平喘、利水消肿 → 请答药名","direction":"effects→name"},
{"id":"card-guizhi-nature","prompt":"药名：桂枝 → 请答性味归经（原样字段，不可同义替换）","direction":"name→nature"},
{"id":"card-shigao-effects","prompt":"功效：清热泻火、除烦止渴 → 请答药名","direction":"effects→name"}
]}"""

    private const val SAMPLE_ANSWER =
        """{"answer":"麻黄","accept":["麻黄","草麻黄","麻黄（龙沙）"],"boundary":"性味归经、用量属原样字段，禁止同义替换，判分时以教材原文为准"}"""

    private const val SAMPLE_UPSTREAM_STATUS =
        """{"app_repo":"morangsu/mneme","public_repo":"morangsu/mneme-data-public","private_repo":"morangsu/mneme-data-private","schema_id":"materia-medica","upstream_rev":7,"local_rev":9,"pending_conflicts":4,"targets":["github-private","github-public","local"]}"""

    private const val SAMPLE_ENTITY_DETAIL =
        """{"id":"herb-mahuang","kind":"herb","primary":"name","title":"麻黄","rev":3,"updated_at":0,"device":"android",
"fields":[
{"key":"name","label":"药名","kind":"text","immutable":false,"required":true,"value":"麻黄"},
{"key":"effects","label":"功效","kind":"tags","immutable":false,"required":false,"value":"发汗解表、宣肺平喘"},
{"key":"nature","label":"性味归经","kind":"text","immutable":true,"required":false,"value":"辛、微苦，温。归肺、膀胱经"},
{"key":"dosage","label":"用量","kind":"text","immutable":true,"required":false,"value":"2–10g"}
]}"""

    /** PullPlan 样例：4 条冲突，覆盖 personalized / immutable / 普通 / 上游移除 四类。 */
    private const val SAMPLE_PULL_PLAN =
        """{
"id":"plan-9c1e7b",
"from_rev":6,
"to_rev":7,
"public_checksum":"3f8a1c09d4e2b7f60a5c8d1e3f4b2a9c07d6e5f1a2b3c4d5e6f708192a3b4c",
"created_at":1731139200000,
"summary":"上游 r6 -> r7：自动合并 5 项 / 新增 1 / 移除 0 / 待裁决 4 项",
"auto":[{"entity_id":"zhimu","field":"effects","from":"清热泻火,滋阴润燥","to":"清热泻火,滋阴润燥,除骨蒸","reason":"本地未改动，自动采用上游"}],
"additions":[{"id":"lugen","name":"芦根"}],
"removals":[],
"conflicts":[
{"id":"cf-1","entity_id":"mahuang","entity_name":"麻黄","field":"canonical","label":"教材主表述","base":"发汗解表，宣肺平喘，利水消肿","upstream":"发汗解表，宣肺平喘，利水消肿；善治风水浮肿","local":"我整理的：发汗力强，先煮去沫","immutable":false,"personalized":true,"suggestion":"KeepMine","kind":"BothChanged","page":12},
{"id":"cf-2","entity_id":"mahuang","entity_name":"麻黄","field":"nature","label":"性味归经","base":"辛、微苦，温。归肺、膀胱经","upstream":"辛、微苦，温。归肺、膀胱、大肠经","local":"辛微苦温，归肺与膀胱（我按讲义补了浮肿）","immutable":true,"personalized":false,"suggestion":"KeepMine","kind":"ImmutableClash","page":12},
{"id":"cf-3","entity_id":"guizhi","entity_name":"桂枝","field":"effects","label":"功效","base":"发汗解肌，温通经脉，助阳化气","upstream":"发汗解肌，温通经脉，助阳化气，平冲降气","local":"发汗解肌，温通经脉","immutable":false,"personalized":false,"suggestion":"TakeUpstream","kind":"BothChanged","page":28},
{"id":"cf-4","entity_id":"zisu","entity_name":"紫苏","field":"*","label":"整条记录","base":"公共库中存在","upstream":"公共库已移除","local":"本地保留且改过","immutable":false,"personalized":true,"suggestion":"KeepMine","kind":"UpstreamRemovedLocallyEdited","page":35}
]
}"""
}

// ---------------------------------------------------------------------------
// 唯一桥接点：默认走 stub；接入真实 uniffi 时只替换这里的方法体。
// ---------------------------------------------------------------------------
object MnemeBridge {

    fun call(method: String, args: Map<String, Any?>): String {
        return try {
            when (method) {
                "openDatabase" -> MnemeApi.openDatabase(
                    args["path"] as String, args["device"] as String
                )
                "state" -> MnemeApi.state()
                "views" -> MnemeApi.views()
                "view" -> MnemeApi.view(args["id"] as String)
                "reverse" -> MnemeApi.reverse(args["cluesJson"] as String)
                "quiz" -> MnemeApi.quiz(args["limit"] as UInt)
                "quizQueue" -> MnemeApi.quizQueue(
                    args["newLimit"] as UInt, args["dueLimit"] as UInt
                )
                "suggestGrade" -> MnemeApi.suggestGrade(
                    args["correct"] as Boolean,
                    (args["elapsedMs"] as Number).toLong(),
                    args["score"] as Double,
                )
                "weakCards" -> MnemeApi.weakCards(args["limit"] as UInt)
                "quizPlan" -> MnemeApi.quizPlan()
                "reviewConfig" -> MnemeApi.reviewConfig()
                "setReviewConfig" -> MnemeApi.setReviewConfig(args["configJson"] as String)
                "answer" -> MnemeApi.answer(args["cardId"] as String)
                "review" -> MnemeApi.review(args["cardId"] as String, args["grade"] as UByte)
                "judge" -> MnemeApi.judge(args["cardId"] as String, args["input"] as String)
                "sync" -> MnemeApi.sync(args["targetId"] as String, args["dry"] as Boolean)
                "upstreamPull" -> MnemeApi.upstreamPull()
                "upstreamResolve" -> MnemeApi.upstreamResolve(
                    args["planId"] as String, args["decisionsJson"] as String
                )
                "upstreamStatus" -> MnemeApi.upstreamStatus()
                "upstreamAdopt" -> MnemeApi.upstreamAdopt()
                "setSchema" -> MnemeApi.setSchema(args["toml"] as String)
                "entityDetail" -> MnemeApi.entityDetail(args["entityId"] as String)
                "editEntity" -> MnemeApi.editEntity(
                    args["entityId"] as String, args["fieldsJson"] as String
                )
                "entityTemplate" -> MnemeApi.entityTemplate(args["kind"] as String)
                "createEntity" -> MnemeApi.createEntity(
                    args["kind"] as String,
                    args["fieldsJson"] as String,
                    args["idHint"] as String,
                )
                "deleteEntity" -> MnemeApi.deleteEntity(args["entityId"] as String)
                "restoreEntity" -> MnemeApi.restoreEntity(args["entityId"] as String)
                "listDeleted" -> MnemeApi.listDeleted()
                "entityHistory" -> MnemeApi.entityHistory(
                    args["entityId"] as String, args["limit"] as UInt
                )
                "undoEdit" -> MnemeApi.undoEdit(args["entityId"] as String)
                "configureSync" -> MnemeApi.configureSync(args["configJson"] as String)
                "importData" -> MnemeApi.importData(
                    args["content"] as String,
                    args["filename"] as String,
                    args["dry"] as Boolean
                )
                "exportData" -> MnemeApi.exportData(args["format"] as String)
                else -> JSONObject().put("error", "未知 FFI 方法：$method").toString()
            }
        } catch (e: UnsatisfiedLinkError) {
            // 尚未放入 libmneme_ffi.so 时退回样例数据，UI 依然可用
            MnemeApiStub.call(method, args)
        } catch (e: Exception) {
            JSONObject().put("error", e.message ?: "FFI 调用失败").toString()
        }
    }
}

// ---------------------------------------------------------------------------
// 面向 UI 的封装：全部 suspend、Dispatchers.IO、统一返回 JSONObject、统一错误 JSON。
// ---------------------------------------------------------------------------
object Api {

    private suspend fun call(method: String, args: Map<String, Any?> = emptyMap()): JSONObject =
        withContext(Dispatchers.IO) {
            try {
                val raw = MnemeBridge.call(method, args).trim()
                when {
                    raw.startsWith("{") -> JSONObject(raw)
                    raw.startsWith("[") -> JSONObject().put("data", JSONArray(raw))
                    else -> JSONObject().put("raw", raw)
                }
            } catch (e: Exception) {
                JSONObject().put(
                    "error",
                    "${e.javaClass.simpleName}: ${e.message ?: "调用 $method 失败"}",
                )
            }
        }

    suspend fun openDatabase(path: String, device: String): JSONObject =
        call("openDatabase", mapOf("path" to path, "device" to device))

    suspend fun state(): JSONObject = call("state")

    suspend fun views(): JSONObject = call("views")

    suspend fun view(id: String): JSONObject = call("view", mapOf("id" to id))

    suspend fun reverse(cluesJson: String): JSONObject =
        call("reverse", mapOf("cluesJson" to cluesJson))

    suspend fun quiz(limit: UInt): JSONObject = call("quiz", mapOf("limit" to limit))

    suspend fun quizQueue(newLimit: UInt = 10u, dueLimit: UInt = 50u): JSONObject =
        call("quizQueue", mapOf("newLimit" to newLimit, "dueLimit" to dueLimit))

    suspend fun suggestGrade(correct: Boolean, elapsedMs: Long, score: Double): JSONObject =
        call("suggestGrade", mapOf("correct" to correct, "elapsedMs" to elapsedMs, "score" to score))

    suspend fun weakCards(limit: UInt = 20u): JSONObject =
        call("weakCards", mapOf("limit" to limit))

    suspend fun quizPlan(): JSONObject = call("quizPlan")

    suspend fun reviewConfig(): JSONObject = call("reviewConfig")

    suspend fun setReviewConfig(configJson: String): JSONObject =
        call("setReviewConfig", mapOf("configJson" to configJson))

    suspend fun answer(cardId: String): JSONObject = call("answer", mapOf("cardId" to cardId))

    suspend fun review(cardId: String, grade: UByte): JSONObject =
        call("review", mapOf("cardId" to cardId, "grade" to grade))

    suspend fun judge(cardId: String, input: String): JSONObject =
        call("judge", mapOf("cardId" to cardId, "input" to input))

    suspend fun sync(targetId: String, dry: Boolean): JSONObject =
        call("sync", mapOf("targetId" to targetId, "dry" to dry))

    suspend fun upstreamPull(): JSONObject = call("upstreamPull")

    suspend fun upstreamResolve(planId: String, decisionsJson: String): JSONObject =
        call("upstreamResolve", mapOf("planId" to planId, "decisionsJson" to decisionsJson))

    suspend fun upstreamStatus(): JSONObject = call("upstreamStatus")

    /** 安装领域包（TOML 原文）。首次启动由 Bundled 清单里用户选择后调用。 */
    suspend fun setSchema(toml: String): JSONObject =
        call("setSchema", mapOf("toml" to toml))

    /** 写入 sync.json 并重建同步引擎；configJson 为 SyncConfig 的 JSON。 */
    suspend fun configureSync(configJson: String): JSONObject =
        call("configureSync", mapOf("configJson" to configJson))

    suspend fun entityDetail(entityId: String): JSONObject =
        call("entityDetail", mapOf("entityId" to entityId))

    /** 批量改字段；fieldsJson 形如 {"effects":"a|b"}。immutable 字段会被 Rust 侧拒绝。 */
    suspend fun editEntity(entityId: String, fieldsJson: String): JSONObject =
        call("editEntity", mapOf("entityId" to entityId, "fieldsJson" to fieldsJson))

    suspend fun entityTemplate(kind: String): JSONObject =
        call("entityTemplate", mapOf("kind" to kind))

    suspend fun createEntity(kind: String, fieldsJson: String, idHint: String = ""): JSONObject =
        call("createEntity", mapOf("kind" to kind, "fieldsJson" to fieldsJson, "idHint" to idHint))

    suspend fun deleteEntity(entityId: String): JSONObject =
        call("deleteEntity", mapOf("entityId" to entityId))

    suspend fun restoreEntity(entityId: String): JSONObject =
        call("restoreEntity", mapOf("entityId" to entityId))

    suspend fun listDeleted(): JSONObject = call("listDeleted")

    suspend fun entityHistory(entityId: String, limit: UInt = 50u): JSONObject =
        call("entityHistory", mapOf("entityId" to entityId, "limit" to limit))

    suspend fun undoEdit(entityId: String): JSONObject =
        call("undoEdit", mapOf("entityId" to entityId))

    suspend fun importData(content: String, filename: String, dry: Boolean): JSONObject =
        call("importData", mapOf("content" to content, "filename" to filename, "dry" to dry))

    suspend fun exportData(format: String): JSONObject =
        call("exportData", mapOf("format" to format))
}
