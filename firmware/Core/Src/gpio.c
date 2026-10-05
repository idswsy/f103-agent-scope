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
  * @file    gpio.c
  * @brief   This file provides code for the configuration
  *          of all used GPIO pins.
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
#include "gpio.h"

/* USER CODE BEGIN 0 */
#include "tim.h"
#include "tft.h"
#include "adc.h"
/* USER CODE END 0 */

/*----------------------------------------------------------------------------*/
/* Configure GPIO                                                             */
/*----------------------------------------------------------------------------*/
/* USER CODE BEGIN 1 */
static uint8_t key1_state = 0;
static uint8_t key2_state = 0;
static uint8_t key3_state = 0;
static uint8_t A_cnt = 0;
static uint8_t B_Value = 0;
/* USER CODE END 1 */

/** Configure pins as
        * Analog
        * Input
        * Output
        * EVENT_OUT
        * EXTI
*/
void MX_GPIO_Init(void)
{

  GPIO_InitTypeDef GPIO_InitStruct = {0};

  /* GPIO Ports Clock Enable */
  __HAL_RCC_GPIOC_CLK_ENABLE();
  __HAL_RCC_GPIOD_CLK_ENABLE();
  __HAL_RCC_GPIOA_CLK_ENABLE();
  __HAL_RCC_GPIOB_CLK_ENABLE();

  /*Configure GPIO pin Output Level */
  HAL_GPIO_WritePin(GPIOC, GPIO_PIN_14|GPIO_PIN_15, GPIO_PIN_SET);

  /*Configure GPIO pin Output Level */
  HAL_GPIO_WritePin(GPIOB, GPIO_PIN_5|GPIO_PIN_6|GPIO_PIN_7|GPIO_PIN_8, GPIO_PIN_SET);

  /*Configure GPIO pins : PC14 PC15 */
  GPIO_InitStruct.Pin = GPIO_PIN_14|GPIO_PIN_15;
  GPIO_InitStruct.Mode = GPIO_MODE_OUTPUT_PP;
  GPIO_InitStruct.Pull = GPIO_NOPULL;
  GPIO_InitStruct.Speed = GPIO_SPEED_FREQ_LOW;
  HAL_GPIO_Init(GPIOC, &GPIO_InitStruct);

  /*Configure GPIO pins : PB13 PB14 PB15 */
  GPIO_InitStruct.Pin = GPIO_PIN_13|GPIO_PIN_14|GPIO_PIN_15;
  GPIO_InitStruct.Mode = GPIO_MODE_IT_FALLING;
  GPIO_InitStruct.Pull = GPIO_PULLUP;
  HAL_GPIO_Init(GPIOB, &GPIO_InitStruct);

  /*Configure GPIO pins : PB3 PB9 */
  GPIO_InitStruct.Pin = GPIO_PIN_3|GPIO_PIN_9;
  GPIO_InitStruct.Mode = GPIO_MODE_INPUT;
  GPIO_InitStruct.Pull = GPIO_PULLUP;
  HAL_GPIO_Init(GPIOB, &GPIO_InitStruct);

  /*Configure GPIO pin : PB4 */
  GPIO_InitStruct.Pin = GPIO_PIN_4;
  GPIO_InitStruct.Mode = GPIO_MODE_IT_RISING_FALLING;
  GPIO_InitStruct.Pull = GPIO_PULLUP;
  HAL_GPIO_Init(GPIOB, &GPIO_InitStruct);

  /*Configure GPIO pins : PB5 PB6 PB7 PB8 */
  GPIO_InitStruct.Pin = GPIO_PIN_5|GPIO_PIN_6|GPIO_PIN_7|GPIO_PIN_8;
  GPIO_InitStruct.Mode = GPIO_MODE_OUTPUT_PP;
  GPIO_InitStruct.Pull = GPIO_NOPULL;
  GPIO_InitStruct.Speed = GPIO_SPEED_FREQ_LOW;
  HAL_GPIO_Init(GPIOB, &GPIO_InitStruct);

  /* EXTI interrupt init*/
  HAL_NVIC_SetPriority(EXTI4_IRQn, 11, 0);
  HAL_NVIC_EnableIRQ(EXTI4_IRQn);

  HAL_NVIC_SetPriority(EXTI15_10_IRQn, 2, 0);
  HAL_NVIC_EnableIRQ(EXTI15_10_IRQn);

}

/* USER CODE BEGIN 2 */

void Open_Led(uint8_t value)
{
	switch(value)
	{
			case LED1:
					HAL_GPIO_WritePin(GPIOC,GPIO_PIN_14,GPIO_PIN_RESET);
					break;
			case LED2:
					HAL_GPIO_WritePin(GPIOC,GPIO_PIN_15,GPIO_PIN_RESET);
					break;
			default:
					break;
	}
}

void CLose_LED(uint8_t value)
{
    switch(value)
    {
        case LED1:
						HAL_GPIO_WritePin(GPIOC,GPIO_PIN_14,GPIO_PIN_SET);
            break;
        case LED2:
						HAL_GPIO_WritePin(GPIOC,GPIO_PIN_15,GPIO_PIN_SET);
            break;
        default:
            break;
    }
}

void Toggle_Led(uint8_t value)
{
		switch(value)
		{
				case LED1:
						HAL_GPIO_TogglePin(GPIOC,GPIO_PIN_14);
						break;
				case LED2:
						HAL_GPIO_TogglePin(GPIOC,GPIO_PIN_15);
						break;
				default:
						break;
		}
}

void Key_Handle(volatile struct Oscilloscope *value)
{
	ADC_ChannelConfTypeDef sConfig = {0};
	float tempValue=0;
	switch((*value).keyValue)
	{
		case KEY1PRESS:
		{
			(*value).pwmOut=((uint32_t)((*value).timerPeriod*0.04f))+(*value).pwmOut;
			if((*value).pwmOut > (*value).timerPeriod)
			{
					(*value).pwmOut = 0;
			}
			__HAL_TIM_SetCompare(&htim2, TIM_CHANNEL_3, (uint16_t)(*value).pwmOut);
		}
			break;
		case KEY2PRESS:
		{
			if((*value).ouptputbit == 0)
			{
				(*value).ouptputbit = 1;
				HAL_TIM_PWM_Start(&htim2,TIM_CHANNEL_3);
				//
			}
			else
			{
				(*value).ouptputbit = 0;
				HAL_TIM_PWM_Stop(&htim2,TIM_CHANNEL_3);
				//HAL_TIM_IC_Stop_IT(&htim3,TIM_CHANNEL_1);
			}
		}
			break;
		case KEY3PRESS:
		{			
			tempValue=(*value).pwmOut/((*value).timerPeriod+0.0f);
			(*value).timerPeriod = (*value).timerPeriod/2;
			if((*value).timerPeriod < 250)
			{
					(*value).timerPeriod = 1000;
			}
			(*value).outputFreq=1000000/(*value).timerPeriod;
			(*value).pwmOut=(uint16_t)((*value).timerPeriod*tempValue);
			__HAL_TIM_SetCompare(&htim2, TIM_CHANNEL_3, (uint16_t)(*value).pwmOut);
			__HAL_TIM_SetAutoreload(&htim2, (uint16_t)((*value).timerPeriod-1));
		}
			break;
		case KEYAPRESS:
		{
			switch((*value).sampletime)
			{
				case ADC_SAMPLETIME_239CYCLES_5:
					(*value).sampletime = ADC_SAMPLETIME_71CYCLES_5;
					break;
				case ADC_SAMPLETIME_71CYCLES_5:
					(*value).sampletime = ADC_SAMPLETIME_55CYCLES_5;
					break;
				case ADC_SAMPLETIME_55CYCLES_5:
					(*value).sampletime = ADC_SAMPLETIME_41CYCLES_5;
					break;
				case ADC_SAMPLETIME_41CYCLES_5:
					(*value).sampletime = ADC_SAMPLETIME_28CYCLES_5;
					break;
				case ADC_SAMPLETIME_28CYCLES_5:
					(*value).sampletime = ADC_SAMPLETIME_239CYCLES_5;
					break;
				default:
					(*value).sampletime=ADC_SAMPLETIME_239CYCLES_5;
					break;
			}
			sConfig.Channel = ADC_CHANNEL_3;
			sConfig.Rank = ADC_REGULAR_RANK_1;
			sConfig.SamplingTime = (*value).sampletime;
			if (HAL_ADC_ConfigChannel(&hadc1, &sConfig) != HAL_OK)
			{
				Error_Handler();
			}
		}
			break;
		case KEYBPRESS:
		{
			switch((*value).sampletime)
			{
				case ADC_SAMPLETIME_239CYCLES_5:
					(*value).sampletime = ADC_SAMPLETIME_28CYCLES_5;
					break;
				case ADC_SAMPLETIME_71CYCLES_5:
					(*value).sampletime = ADC_SAMPLETIME_239CYCLES_5;
					break;
				case ADC_SAMPLETIME_55CYCLES_5:
					(*value).sampletime = ADC_SAMPLETIME_71CYCLES_5;
					break;
				case ADC_SAMPLETIME_41CYCLES_5:
					(*value).sampletime = ADC_SAMPLETIME_55CYCLES_5;
					break;
				case ADC_SAMPLETIME_28CYCLES_5:
					(*value).sampletime = ADC_SAMPLETIME_41CYCLES_5;
					break;
				default:
					(*value).sampletime=ADC_SAMPLETIME_239CYCLES_5;
					break;
			}
			sConfig.Channel = ADC_CHANNEL_3;
			sConfig.Rank = ADC_REGULAR_RANK_1;
			sConfig.SamplingTime = (*value).sampletime;
			if (HAL_ADC_ConfigChannel(&hadc1, &sConfig) != HAL_OK)
			{
				Error_Handler();
			}
		}
			break;
		default:
			break;
	}
	(*value).keyValue=NoPRESS;
	TFT_ShowUI(value); 
}
void Key_Sacnf(volatile struct Oscilloscope *value)
{
	if(key1_state == KEYPRESS){
		HAL_Delay(20);
		if(HAL_GPIO_ReadPin(GPIOB,GPIO_PIN_13) == GPIO_PIN_RESET){
			(*value).keyValue = KEY1PRESS;
			key1_state = NoPRESS;
		}
	}
	else{
		key1_state = NoPRESS;
	}
	
	if(key2_state == KEYPRESS){
		HAL_Delay(20);
		if(HAL_GPIO_ReadPin(GPIOB,GPIO_PIN_14) == GPIO_PIN_RESET){
			Toggle_Led(LED1);
			(*value).keyValue = KEY2PRESS;
			key2_state = NoPRESS;
		}
	}
	else{
		key2_state = NoPRESS;
	}
	
	if(key3_state == KEYPRESS){
		HAL_Delay(20);
		if(HAL_GPIO_ReadPin(GPIOB,GPIO_PIN_15) == GPIO_PIN_RESET){
			(*value).keyValue = KEY3PRESS;
			key3_state = NoPRESS;
		}
	}
	else{
		key3_state = NoPRESS;
	}
}

void KEYD_SCAN(volatile struct Oscilloscope *value)
{
	if(HAL_GPIO_ReadPin(GPIOB,GPIO_PIN_9) == GPIO_PIN_RESET){
		HAL_Delay(20);
		if(HAL_GPIO_ReadPin(GPIOB,GPIO_PIN_9) == GPIO_PIN_RESET){
			while(HAL_GPIO_ReadPin(GPIOB,GPIO_PIN_9) == GPIO_PIN_RESET);
			(*value).keyValue = KEYDPRESS;
		}
	}
}

void HAL_GPIO_EXTI_Callback(uint16_t GPIO_Pin)
{
	if(GPIO_Pin == GPIO_PIN_4)
	{
		if((HAL_GPIO_ReadPin(GPIOB,GPIO_PIN_4) == GPIO_PIN_RESET) && (A_cnt == 0))	//A���½��ش���һ��
		{
			A_cnt++;
			B_Value = 0;
			if(HAL_GPIO_ReadPin(GPIOB,GPIO_PIN_3) == GPIO_PIN_SET){
				B_Value = 1;
			}
		}
		else if((HAL_GPIO_ReadPin(GPIOB,GPIO_PIN_4) == GPIO_PIN_SET) && (A_cnt == 1))	//A�������ش���һ��
		{
			A_cnt = 0;
			A_cnt = 0;
			if((B_Value == 1) && (HAL_GPIO_ReadPin(GPIOB,GPIO_PIN_3) == GPIO_PIN_RESET)){
				oscilloscope.keyValue=KEYBPRESS; 
			}
			if((B_Value == 0) && (HAL_GPIO_ReadPin(GPIOB,GPIO_PIN_3) == GPIO_PIN_SET)){
				oscilloscope.keyValue=KEYAPRESS;
			}
		}
	}
	if(GPIO_Pin == GPIO_PIN_13)
	{
		if(HAL_GPIO_ReadPin(GPIOB,GPIO_PIN_13) == GPIO_PIN_RESET){
			key1_state = KEYPRESS;
		}
	}
	if(GPIO_Pin == GPIO_PIN_14)
	{
		if(HAL_GPIO_ReadPin(GPIOB,GPIO_PIN_14) == GPIO_PIN_RESET){
			key2_state = KEYPRESS;
		}
	}
	if(GPIO_Pin == GPIO_PIN_15)
	{
		if(HAL_GPIO_ReadPin(GPIOB,GPIO_PIN_15) == GPIO_PIN_RESET){
			key3_state = KEYPRESS;
		}
	}
}
/* USER CODE END 2 */
