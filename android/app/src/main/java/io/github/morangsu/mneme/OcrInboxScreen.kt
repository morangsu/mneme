package io.github.morangsu.mneme

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.google.mlkit.vision.common.InputImage
import com.google.mlkit.vision.text.TextRecognition
import com.google.mlkit.vision.text.chinese.ChineseTextRecognizerOptions

/**
 * OCR 收件箱：拍照 → ML Kit 中文识别 → 文本回填到导入框 → 由用户确认后调 Api.importData。
 * 注意：生僻字（燀、煨、烊化、十八反十九畏等）必须在导入前人工校对，OCR 不代替核对教材原文。
 */
@Composable
fun OcrInbox(
    modifier: Modifier = Modifier,
    onText: (String) -> Unit,
) {
    var status by remember { mutableStateOf("") }

    val takePicture = rememberLauncherForActivityResult(
        ActivityResultContracts.TakePicturePreview()
    ) { bitmap ->
        if (bitmap == null) {
            status = "未获取到图片"
            return@rememberLauncherForActivityResult
        }
        val image = InputImage.fromBitmap(bitmap, 0)
        val recognizer = TextRecognition.getClient(ChineseTextRecognizerOptions.Builder().build())
        recognizer.process(image)
            .addOnSuccessListener { result ->
                status = "识别到 ${result.text.length} 字，已回填导入框（请人工校对生僻字）"
                onText(result.text)
            }
            .addOnFailureListener { e ->
                status = "识别失败：${e.message ?: "未知错误"}"
            }
            .addOnCompleteListener {
                recognizer.close()
            }
    }

    Column(modifier = modifier, verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Button(
            onClick = { takePicture.launch(null) },
            colors = ButtonDefaults.buttonColors(
                containerColor = MnemeColors.PanelAlt,
                contentColor = MnemeColors.Accent,
            ),
        ) {
            Text("拍照识别 OCR", fontSize = 13.sp)
        }
        if (status.isNotEmpty()) Dim(status)
    }
}
