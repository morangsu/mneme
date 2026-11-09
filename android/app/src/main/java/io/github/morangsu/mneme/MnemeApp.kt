package io.github.morangsu.mneme

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.GridView
import androidx.compose.material.icons.filled.MergeType
import androidx.compose.material.icons.filled.School
import androidx.compose.material.icons.filled.Search
import androidx.compose.material.icons.filled.Sync
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.NavigationBarItemDefaults
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.font.FontWeight

/**
 * 五个底部标签。
 * 用状态切换而非 navigation-compose：依赖更少、离线可控；各 Screen 之间仅通过回调协作。
 */
enum class Tab(val label: String, val icon: ImageVector) {
    Matrix("矩阵", Icons.Filled.GridView),
    Reverse("反查", Icons.Filled.Search),
    Quiz("复习", Icons.Filled.School),
    Sync("同步", Icons.Filled.Sync),
    Conflict("冲突", Icons.Filled.MergeType),
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun MnemeApp() {
    var tab by remember { mutableStateOf(Tab.Matrix) }
    // 拉取公共库后跳到冲突页时递增，用于触发 ConflictScreen 重新拉取计划。
    var conflictEpoch by remember { mutableIntStateOf(0) }

    Scaffold(
        containerColor = MnemeColors.Bg,
        topBar = {
            TopAppBar(
                title = {
                    Text(
                        "mneme · ${tab.label}",
                        color = MnemeColors.Text,
                        fontWeight = FontWeight.SemiBold,
                    )
                },
                colors = TopAppBarDefaults.topAppBarColors(
                    containerColor = MnemeColors.Panel,
                    titleContentColor = MnemeColors.Text,
                ),
            )
        },
        bottomBar = {
            NavigationBar(containerColor = MnemeColors.Panel) {
                Tab.entries.forEach { t ->
                    NavigationBarItem(
                        selected = tab == t,
                        onClick = { tab = t },
                        icon = { Icon(t.icon, contentDescription = t.label) },
                        label = { Text(t.label) },
                        colors = NavigationBarItemDefaults.colors(
                            selectedIconColor = MnemeColors.Accent,
                            selectedTextColor = MnemeColors.Accent,
                            unselectedIconColor = MnemeColors.TextDim,
                            unselectedTextColor = MnemeColors.TextDim,
                            indicatorColor = MnemeColors.PanelAlt,
                        ),
                    )
                }
            }
        },
    ) { padding ->
        Box(
            modifier = Modifier
                .padding(padding)
                .fillMaxSize(),
        ) {
            when (tab) {
                Tab.Matrix -> MatrixScreen()
                Tab.Reverse -> ReverseScreen()
                Tab.Quiz -> QuizScreen()
                Tab.Sync -> SyncScreen(
                    onGoConflict = {
                        conflictEpoch++
                        tab = Tab.Conflict
                    },
                )
                Tab.Conflict -> ConflictScreen(epoch = conflictEpoch)
            }
        }
    }
}
