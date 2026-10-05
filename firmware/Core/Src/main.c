/*
 * Upstream notice -- the USER CODE sections of this CubeMX-generated file
 * contain code from the "STM32-Oscilloscope" project (simple digital
 * oscilloscope firmware), redistributed here under the MulanPSL-2.0 license.
 * The generated sections remain under STMicroelectronics' BSD-3-Clause terms.
 *
 * License text:  http://license.coscl.org.cn/MulanPSL2
 * Provenance:    see NOTICE.md, section 2.
 */

/* USER CODE BEGIN Header */


/**
  ******************************************************************************
  * @file           : main.c
  * @brief          : Main program body
  ******************************************************************************
  * @attention
  *
  * Copyright (c) 2024 STMicroelectronics.
  * All rights reserved.
  *
  * This software is licensed under terms that can be found in the LICENSE file
  * in the root directory of this software component.
  * If no LICENSE file comes with this software, it is provided AS-IS.
  *
  ******************************************************************************
  */
/* USER CODE END Header */
/* Includes ------------------------------------------------------------------*/
#include "main.h"
#include "adc.h"
#include "dma.h"
#include "spi.h"
#include "tim.h"
#include "gpio.h"

/* Private includes ----------------------------------------------------------*/
/* USER CODE BEGIN Includes */
#include "hal.h"
#include "acq.h"
#include "proto_task.h"
#include "hal_impl.h"
#include "usart.h"
#include "tft.h"
#include "tft_init.h"
/* USER CODE END Includes */

/* Private typedef -----------------------------------------------------------*/
/* USER CODE BEGIN PTD */

/* USER CODE END PTD */

/* Private define ------------------------------------------------------------*/
/* USER CODE BEGIN PD */

/* USER CODE END PD */

/* Private macro -------------------------------------------------------------*/
/* USER CODE BEGIN PM */

/* USER CODE END PM */

/* Private variables ---------------------------------------------------------*/

/* USER CODE BEGIN PV */
/* 应用层的两个状态机。放在文件作用域是因为它们要活到整个程序生命周期；
 * 初始化在 `main()` 的 USER CODE 2 段（见那里）。 */
static acq_t        g_acq;
static proto_task_t g_pt;
/* 上游的应用逻辑已移到 Hardware/src/scope_ui.c（`#if SCOPE_LOCAL_UI`，默认关）。 */
/* USER CODE END PV */

/* Private function prototypes -----------------------------------------------*/
void SystemClock_Config(void);
/* USER CODE BEGIN PFP */
/* 上游的应用逻辑已移到 Hardware/src/scope_ui.c（`#if SCOPE_LOCAL_UI`，默认关）。 */
/* USER CODE END PFP */

/* Private user code ---------------------------------------------------------*/
/* USER CODE BEGIN 0 */
/* 上游的应用逻辑已移到 Hardware/src/scope_ui.c（`#if SCOPE_LOCAL_UI`，默认关）。 */
/* USER CODE END 0 */

/**
  * @brief  The application entry point.
  * @retval int
  */
int main(void)
{
  /* USER CODE BEGIN 1 */
/* 上游的应用逻辑已移到 Hardware/src/scope_ui.c（`#if SCOPE_LOCAL_UI`，默认关）。 */
  /* USER CODE END 1 */

  /* MCU Configuration--------------------------------------------------------*/

  /* Reset of all peripherals, Initializes the Flash interface and the Systick. */
  HAL_Init();

  /* USER CODE BEGIN Init */
	
  /* USER CODE END Init */

  /* Configure the system clock */
  SystemClock_Config();

  /* USER CODE BEGIN SysInit */

  /* USER CODE END SysInit */

  /* Initialize all configured peripherals */
  MX_GPIO_Init();
  MX_DMA_Init();
  MX_ADC1_Init();
  MX_SPI1_Init();
  MX_TIM2_Init();
  MX_TIM3_Init();
  MX_TIM4_Init();   /* ADC 采样触发。与 MX_ADC1_Init 的先后无所谓 ——
                     * 这里只是写寄存器；ADC 真正开始跑要等 HAL_ADC_Start_DMA。 */
  MX_USART1_UART_Init();
  /* USER CODE BEGIN 2 */
  /* 应用层在这里接上。App/ 是硬件无关的（ADR-008），硬件能力由
   * `HalImpl_Get()` 交进去 —— 装配点只有一处，见 Hardware/hal_impl.c。 */
  {
    const hal_t *hal = HalImpl_Get();
    HalImpl_Init();                    /* µs 时基、收发环、点起第一次接收中断 */
    acq_init(&g_acq, hal);
    proto_task_init(&g_pt, hal, &g_acq);
  }
  /* USER CODE END 2 */

  /* Infinite loop */
  /* USER CODE BEGIN WHILE */
  while (1)
  {
    /* USER CODE END WHILE */

    /* USER CODE BEGIN 3 */
    /* 先收命令：`proto_task_poll` 自己会把「能收多少收多少、能处理多少处理多少」
     * 做完，且**绝不阻塞**（见 App/proto_task.h）。 */
    proto_task_poll(&g_pt);

    /* 再推进采集状态机。`acq_poll` 一轮只回一个事件，所以这里排空它。
     *
     * ⚠ 循环**有上限**：万一 `acq_poll` 出于任何原因不返回 NONE，
     *   无上限的排空会把上面那句 `proto_task_poll` 永远饿死 ——
     *   表现是串口突然哑掉，而且没有任何迹象。上限 4 只是兜底，
     *   正常路径一轮最多一两个事件。 */
    for (int guard = 0; guard < 4; guard++) {
        acq_out_t out;
        if (acq_poll(&g_acq, &out) == ACQ_EV_NONE) {
            break;
        }
        proto_task_emit(&g_pt, &out);
    }
  /* USER CODE END 3 */
  }
}

/**
  * @brief System Clock Configuration
  * @retval None
  */
void SystemClock_Config(void)
{
  RCC_OscInitTypeDef RCC_OscInitStruct = {0};
  RCC_ClkInitTypeDef RCC_ClkInitStruct = {0};
  RCC_PeriphCLKInitTypeDef PeriphClkInit = {0};

  /** Initializes the RCC Oscillators according to the specified parameters
  * in the RCC_OscInitTypeDef structure.
  */
  RCC_OscInitStruct.OscillatorType = RCC_OSCILLATORTYPE_HSE;
  RCC_OscInitStruct.HSEState = RCC_HSE_ON;
  RCC_OscInitStruct.HSEPredivValue = RCC_HSE_PREDIV_DIV1;
  RCC_OscInitStruct.HSIState = RCC_HSI_ON;
  RCC_OscInitStruct.PLL.PLLState = RCC_PLL_ON;
  RCC_OscInitStruct.PLL.PLLSource = RCC_PLLSOURCE_HSE;
  RCC_OscInitStruct.PLL.PLLMUL = RCC_PLL_MUL9;
  if (HAL_RCC_OscConfig(&RCC_OscInitStruct) != HAL_OK)
  {
    Error_Handler();
  }

  /** Initializes the CPU, AHB and APB buses clocks
  */
  RCC_ClkInitStruct.ClockType = RCC_CLOCKTYPE_HCLK|RCC_CLOCKTYPE_SYSCLK
                              |RCC_CLOCKTYPE_PCLK1|RCC_CLOCKTYPE_PCLK2;
  RCC_ClkInitStruct.SYSCLKSource = RCC_SYSCLKSOURCE_PLLCLK;
  RCC_ClkInitStruct.AHBCLKDivider = RCC_SYSCLK_DIV1;
  RCC_ClkInitStruct.APB1CLKDivider = RCC_HCLK_DIV2;
  RCC_ClkInitStruct.APB2CLKDivider = RCC_HCLK_DIV1;

  if (HAL_RCC_ClockConfig(&RCC_ClkInitStruct, FLASH_LATENCY_2) != HAL_OK)
  {
    Error_Handler();
  }
  PeriphClkInit.PeriphClockSelection = RCC_PERIPHCLK_ADC;
  PeriphClkInit.AdcClockSelection = RCC_ADCPCLK2_DIV6;
  if (HAL_RCCEx_PeriphCLKConfig(&PeriphClkInit) != HAL_OK)
  {
    Error_Handler();
  }
}

/* USER CODE BEGIN 4 */
/* 上游的应用逻辑已移到 Hardware/src/scope_ui.c（`#if SCOPE_LOCAL_UI`，默认关）。 */
/* USER CODE END 4 */

/**
  * @brief  This function is executed in case of error occurrence.
  * @retval None
  */
void Error_Handler(void)
{
  /* USER CODE BEGIN Error_Handler_Debug */
  /* User can add his own implementation to report the HAL error return state */
  __disable_irq();
  while (1)
  {
  }
  /* USER CODE END Error_Handler_Debug */
}

#ifdef  USE_FULL_ASSERT
/**
  * @brief  Reports the name of the source file and the source line number
  *         where the assert_param error has occurred.
  * @param  file: pointer to the source file name
  * @param  line: assert_param error line source number
  * @retval None
  */
void assert_failed(uint8_t *file, uint32_t line)
{
  /* USER CODE BEGIN 6 */
  /* User can add his own implementation to report the file name and line number,
     ex: printf("Wrong parameters value: file %s on line %d\r\n", file, line) */
  /* USER CODE END 6 */
}
#endif /* USE_FULL_ASSERT */
